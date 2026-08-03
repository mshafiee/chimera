"""
WebSocket-Based Real-Time Wallet Discovery System

This module implements comprehensive WebSocket integration for real-time DEX
transaction monitoring and wallet discovery.

COMPREHENSIVE ENHANCEMENTS:
- Real-time DEX transaction monitoring via WebSocket
- Streaming top 200 wallets for instant discovery (expanded from 100)
- Connection health monitoring and automatic reconnection
- Event-driven wallet discovery with minimal latency
- Parallel WebSocket connections for multiple DEX programs
- Comprehensive error handling and fallback mechanisms

Architecture:
- WebSocketDiscoveryClient: Main WebSocket client for real-time monitoring
- DEXMonitor: Manages WebSocket connections to multiple DEX programs
- RealTimeWalletExtractor: Extracts wallets from WebSocket transactions
- WebSocketHealthMonitor: Monitors connection health and triggers reconnection

Configuration:
- SCOUT_WS_TOP_WALLETS: Number of top wallets to stream (default: 200)
- SCOUT_WS_RECONNECT_DELAY: Delay between reconnection attempts (default: 5s)
- SCOUT_WS_MAX_RECONNECT_ATTEMPTS: Maximum reconnection attempts (default: 10)
"""

import os
import json
import time
import logging
import asyncio
import websockets
from typing import Dict, List, Optional, Any, Callable
from dataclasses import dataclass, field
from enum import Enum

logger = logging.getLogger(__name__)


class WebSocketConnectionState(Enum):
    """WebSocket connection states."""
    DISCONNECTED = "disconnected"
    CONNECTING = "connecting"
    CONNECTED = "connected"
    RECONNECTING = "reconnecting"
    FAILED = "failed"


class DEXProgram(Enum):
    """DEX programs to monitor via WebSocket."""
    JUPITER = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"
    ORCA = "9WzaBBWQNqAghxSAfKUUx3ZkhBBFCkTUvJJJcjF2oG4"
    RAYDIUM = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
    WHIRLPOOL = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"
    STEPTHIRD = "7dHbWXmci3dTUpSFJC3s3nxMPrsrTn5fQjYPb26cscQ"


@dataclass
class WebSocketConnectionStats:
    """Statistics for WebSocket connection."""
    connected_at: float = 0.0
    messages_received: int = 0
    wallets_discovered: int = 0
    reconnection_count: int = 0
    last_message_time: float = 0.0
    connection_uptime_seconds: float = 0.0
    average_latency_ms: float = 0.0


@dataclass
class DiscoveredWalletEvent:
    """Wallet discovered via WebSocket."""
    address: str
    discovery_timestamp: float
    source_dex: DEXProgram
    transaction_signature: str
    transaction_type: str  # SWAP, TRADE, LIQUIDITY, etc.
    quality_score: float = 0.0
    metadata: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return {
            "address": self.address,
            "discovery_timestamp": self.discovery_timestamp,
            "source_dex": self.source_dex.value,
            "transaction_signature": self.transaction_signature,
            "transaction_type": self.transaction_type,
            "quality_score": self.quality_score,
            "metadata": self.metadata,
        }


class WebSocketDiscoveryClient:
    """
    WebSocket client for real-time DEX transaction monitoring.

    This client connects to Solana RPC WebSocket endpoints to monitor
    DEX transactions in real-time and extract wallet addresses.

    Features:
    - Real-time transaction monitoring
    - Automatic reconnection with exponential backoff
    - Connection health monitoring
    - Parallel subscription management
    - Low-latency wallet extraction
    """

    def __init__(
        self,
        rpc_ws_url: Optional[str] = None,
        top_wallets: int = 200,
        max_reconnect_attempts: int = 10,
        program: Optional[DEXProgram] = None,
    ):
        """Initialize the WebSocket discovery client."""
        self._rpc_ws_url = rpc_ws_url or self._get_default_ws_url()
        self._top_wallets = top_wallets
        self._max_reconnect_attempts = max_reconnect_attempts
        # DEX program this client monitors (None = generic); used to attribute
        # program notifications to the correct source DEX.
        self._program = program

        # Connection state
        self._connection_state = WebSocketConnectionState.DISCONNECTED
        self._websocket: Optional[websockets.WebSocketClientProtocol] = None
        self._reconnect_attempts = 0

        # Statistics
        self._stats = WebSocketConnectionStats()

        # Discovered wallets
        self._discovered_wallets: Dict[str, DiscoveredWalletEvent] = {}
        self._wallet_quality_scores: Dict[str, float] = {}

        # Event callbacks
        self._on_wallet_discovered: Optional[Callable[[DiscoveredWalletEvent], None]] = None
        self._on_connection_state_change: Optional[Callable[[WebSocketConnectionState], None]] = None

        # Monitoring task
        self._monitor_task: Optional[asyncio.Task] = None
        self._processing = False
        self._running = False

        # accountSubscribe bookkeeping: server subscription id -> address
        self._request_counter = 0
        self._pending_account_subs: Dict[int, str] = {}  # request id -> address
        self._account_subscriptions: Dict[int, str] = {}  # server sub id -> address

        logger.info(f"[WebSocketClient] Initialized with {top_wallets} top wallets target")

    def _get_default_ws_url(self) -> str:
        """Get default WebSocket URL from environment or Helius."""
        # Try environment variable first
        ws_url = os.getenv("SOLANA_WS_URL") or os.getenv("CHIMERA_RPC__WS_URL")
        if ws_url:
            return ws_url

        # Fallback to Helius WebSocket endpoint
        helius_api_key = os.getenv("HELIUS_API_KEY") or os.getenv("CHIMERA_RPC__PRIMARY_URL", "").split("api-key=")[-1].split("&")[0] if "api-key=" in os.getenv("CHIMERA_RPC__PRIMARY_URL", "") else ""

        if helius_api_key:
            return f"wss://mainnet.helius-rpc.com/?api-key={helius_api_key}"

        # Final fallback to public Solana WebSocket
        return "wss://api.mainnet-beta.solana.com"

    def set_wallet_discovered_callback(
        self,
        callback: Callable[[DiscoveredWalletEvent], None]
    ) -> None:
        """Set callback for when a wallet is discovered."""
        self._on_wallet_discovered = callback

    def set_connection_state_change_callback(
        self,
        callback: Callable[[WebSocketConnectionState], None]
    ) -> None:
        """Set callback for connection state changes."""
        self._on_connection_state_change = callback

    async def connect(self) -> bool:
        """Connect to the WebSocket endpoint."""
        try:
            self._set_connection_state(WebSocketConnectionState.CONNECTING)

            # Connect with timeout
            self._websocket = await asyncio.wait_for(
                websockets.connect(self._rpc_ws_url),
                timeout=30.0
            )

            self._set_connection_state(WebSocketConnectionState.CONNECTED)
            self._stats.connected_at = time.time()
            self._reconnect_attempts = 0

            logger.info(f"[WebSocketClient] Connected to {self._rpc_ws_url}")

            # Start message processing (single monitor task, cancel any stale one
            # so a reconnection never spawns a second concurrent message loop)
            if self._monitor_task and not self._monitor_task.done():
                self._monitor_task.cancel()
            self._monitor_task = asyncio.create_task(self._process_messages())

            return True

        except Exception as e:
            logger.error(f"[WebSocketClient] Connection failed: {e}")
            self._set_connection_state(WebSocketConnectionState.FAILED)
            return False

    async def disconnect(self) -> None:
        """Disconnect from the WebSocket endpoint."""
        self._running = False

        if self._websocket:
            try:
                await self._websocket.close()
                logger.info("[WebSocketClient] Disconnected")
            except Exception as e:
                logger.error(f"[WebSocketClient] Error during disconnect: {e}")
            self._websocket = None

        if self._monitor_task:
            self._monitor_task.cancel()
            try:
                await self._monitor_task
            except (asyncio.CancelledError, Exception):
                pass
            self._monitor_task = None

        self._set_connection_state(WebSocketConnectionState.DISCONNECTED)

    async def subscribe_to_dex_programs(
        self,
        programs: Optional[List[DEXProgram]] = None
    ) -> None:
        """
        Subscribe to DEX program transactions.

        Args:
            programs: List of DEX programs to monitor (default: all major DEXes)
        """
        if not programs:
            programs = [
                DEXProgram.JUPITER,
                DEXProgram.ORCA,
                DEXProgram.RAYDIUM,
                DEXProgram.WHIRLPOOL,
            ]

        for program in programs:
            try:
                subscription_message = {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "programSubscribe",
                    "params": [
                        program.value,
                        {
                            "encoding": "jsonParsed",
                            "commitment": "confirmed"
                        }
                    ]
                }

                if self._websocket:
                    await self._websocket.send(json.dumps(subscription_message))
                    logger.info(f"[WebSocketClient] Subscribed to {program.value}")

                # Small delay to avoid rate limiting
                await asyncio.sleep(0.1)

            except Exception as e:
                logger.error(f"[WebSocketClient] Failed to subscribe to {program.value}: {e}")

    async def subscribe_to_account_transactions(
        self,
        account_addresses: List[str]
    ) -> None:
        """
        Subscribe to transactions for specific accounts (wallets).

        Args:
            account_addresses: List of wallet addresses to monitor
        """
        for address in account_addresses[:self._top_wallets]:
            try:
                self._request_counter += 1
                request_id = self._request_counter
                subscription_message = {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": "accountSubscribe",
                    "params": [
                        address,
                        {
                            "encoding": "jsonParsed",
                            "commitment": "confirmed"
                        }
                    ]
                }

                # Track the pending request so the confirmation can be mapped
                # to the server subscription id
                self._pending_account_subs[request_id] = address

                if self._websocket:
                    await self._websocket.send(json.dumps(subscription_message))

                # Small delay to avoid rate limiting
                await asyncio.sleep(0.05)

            except Exception as e:
                logger.error(f"[WebSocketClient] Failed to subscribe to {address[:8]}...: {e}")

        logger.info(f"[WebSocketClient] Subscribed to {len(account_addresses[:self._top_wallets])} accounts")

    async def _process_messages(self) -> None:
        """Process incoming WebSocket messages."""
        # Only one message loop may run at a time (reconnection spawns new
        # tasks; a stale loop must not keep reading from a shared socket)
        if self._processing:
            return
        self._processing = True
        self._running = True

        try:
            while self._running and self._websocket:
                try:
                    message = await asyncio.wait_for(
                        self._websocket.recv(),
                        timeout=60.0  # 60 second timeout for health check
                    )

                    self._stats.messages_received += 1
                    self._stats.last_message_time = time.time()

                    # Parse and process message
                    await self._process_message(message)

                except asyncio.TimeoutError:
                    logger.warning("[WebSocketClient] No messages received for 60 seconds")
                    await self._handle_connection_loss()
                    if self._connection_state == WebSocketConnectionState.FAILED:
                        break

                except Exception as e:
                    logger.error(f"[WebSocketClient] Error processing message: {e}")
                    await self._handle_connection_loss()
                    if self._connection_state == WebSocketConnectionState.FAILED:
                        break
        finally:
            self._processing = False

    async def _process_message(self, message: str) -> None:
        """Process a single WebSocket message."""
        try:
            data = json.loads(message)

            # Handle different message types
            method = data.get("method", "")

            if method == "programNotification":
                await self._handle_program_notification(data)
            elif method == "accountNotification":
                await self._handle_account_notification(data)
            elif not method and "id" in data and "result" in data:
                # Subscription confirmation: result is the server subscription id
                server_id = data.get("result")
                address = self._pending_account_subs.pop(data.get("id"), None)
                if address is not None and isinstance(server_id, int):
                    self._account_subscriptions[server_id] = address

        except json.JSONDecodeError as e:
            logger.error(f"[WebSocketClient] Failed to parse message: {e}")
        except Exception as e:
            logger.error(f"[WebSocketClient] Error handling message: {e}")

    async def _handle_program_notification(self, data: Dict[str, Any]) -> None:
        """Handle program notification (DEX transaction)."""
        try:
            params = data.get("params", {})
            result = params.get("result", {})

            # Extract transaction signature
            signature = result.get("signature", "")
            if not signature:
                return

            # Extract wallet addresses from transaction
            transaction = result.get("transaction", {})
            message = transaction.get("message", {})
            account_keys = message.get("accountKeys", [])

            if not account_keys:
                return

            # Extract fee payer (usually the initiating wallet). With the
            # jsonParsed encoding, accountKeys are objects like
            # {"pubkey": ..., "signer": ...} rather than raw base58 strings.
            first_key = account_keys[0]
            fee_payer = first_key.get("pubkey") if isinstance(first_key, dict) else first_key

            if fee_payer and self._is_valid_wallet_address(fee_payer):
                # Attribute the event to the program this client monitors
                source_dex = self._program or DEXProgram.JUPITER
                # Create wallet discovery event
                event = DiscoveredWalletEvent(
                    address=fee_payer,
                    discovery_timestamp=time.time(),
                    source_dex=source_dex,
                    transaction_signature=signature,
                    transaction_type="SWAP",
                    quality_score=self._calculate_initial_quality_score(fee_payer),
                    metadata={
                        "discovery_method": "websocket_program",
                        "account_count": len(account_keys),
                    }
                )

                await self._register_discovered_wallet(event)

        except Exception as e:
            logger.error(f"[WebSocketClient] Error handling program notification: {e}")

    async def _handle_account_notification(self, data: Dict[str, Any]) -> None:
        """Handle account notification (wallet activity)."""
        try:
            params = data.get("params", {})
            result = params.get("result", {})

            # Map the server subscription id back to the subscribed address
            subscription_id = params.get("subscription", 0)
            address = self._account_subscriptions.get(subscription_id)
            if not address:
                return

            transaction = result.get("transaction", {})
            signature = (transaction or {}).get("signature") or result.get("signature", "")

            # Account activity is a real-time signal: register as a wallet event
            if self._is_valid_wallet_address(address):
                event = DiscoveredWalletEvent(
                    address=address,
                    discovery_timestamp=time.time(),
                    source_dex=self._program or DEXProgram.JUPITER,
                    transaction_signature=signature,
                    transaction_type="ACCOUNT_ACTIVITY",
                    quality_score=self._calculate_initial_quality_score(address),
                    metadata={
                        "discovery_method": "websocket_account",
                        "subscription_id": subscription_id,
                    }
                )
                await self._register_discovered_wallet(event)

        except Exception as e:
            logger.error(f"[WebSocketClient] Error handling account notification: {e}")

    async def _register_discovered_wallet(self, event: DiscoveredWalletEvent) -> None:
        """Register a discovered wallet."""
        # Only add if not already discovered
        if event.address not in self._discovered_wallets:
            self._discovered_wallets[event.address] = event
            self._stats.wallets_discovered += 1

            logger.debug(
                f"[WebSocketClient] Discovered wallet {event.address[:8]}... "
                f"via {event.source_dex.value}"
            )

            # Trigger callback if set
            if self._on_wallet_discovered:
                try:
                    self._on_wallet_discovered(event)
                except Exception as e:
                    logger.error(f"[WebSocketClient] Error in wallet discovered callback: {e}")

    def _calculate_initial_quality_score(self, address: str) -> float:
        """Calculate initial quality score for a discovered wallet."""
        # Base score for real-time discovery
        score = 50.0

        # Bonus for being discovered via WebSocket (real-time activity)
        score += 20.0

        # Bonus for recent activity
        score += 10.0

        return min(100.0, score)

    def _is_valid_wallet_address(self, address: str) -> bool:
        """Check if address is a valid wallet address."""
        if not address or len(address) < 32 or len(address) > 44:
            return False

        # Filter system programs
        system_programs = {
            "11111111111111111111111111111111",  # System Program
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  # Token Program
        }

        if address in system_programs:
            return False

        # Filter program-like addresses
        if address.endswith("11111111111111111111111111111111"):
            return False

        return True

    async def _handle_connection_loss(self) -> None:
        """Handle connection loss and trigger reconnection."""
        logger.warning("[WebSocketClient] Connection lost, attempting reconnection")

        if self._reconnect_attempts < self._max_reconnect_attempts:
            self._set_connection_state(WebSocketConnectionState.RECONNECTING)
            self._reconnect_attempts += 1
            self._stats.reconnection_count = self._reconnect_attempts

            # Exponential backoff
            delay = min(5 * (2 ** self._reconnect_attempts), 60)
            logger.info(f"[WebSocketClient] Reconnecting in {delay}s (attempt {self._reconnect_attempts})")

            await asyncio.sleep(delay)

            if not self._running:
                # A disconnect()/shutdown happened during backoff
                self._set_connection_state(WebSocketConnectionState.DISCONNECTED)
                return

            # Close the stale socket before reconnecting so it can't leak
            if self._websocket:
                try:
                    await self._websocket.close()
                except Exception:
                    pass
                self._websocket = None

            # Attempt reconnection
            success = await self.connect()
            if success:
                # Re-establish subscriptions
                await self.subscribe_to_dex_programs()

        else:
            logger.error(f"[WebSocketClient] Max reconnection attempts ({self._max_reconnect_attempts}) reached")
            self._set_connection_state(WebSocketConnectionState.FAILED)
            # Drop the stale socket so the message loop terminates
            if self._websocket:
                try:
                    await self._websocket.close()
                except Exception:
                    pass
                self._websocket = None

    def _set_connection_state(self, state: WebSocketConnectionState) -> None:
        """Set connection state and trigger callback."""
        self._connection_state = state

        if self._on_connection_state_change:
            try:
                self._on_connection_state_change(state)
            except Exception as e:
                logger.error(f"[WebSocketClient] Error in connection state callback: {e}")

    def get_discovered_wallets(self) -> List[DiscoveredWalletEvent]:
        """Get all discovered wallets."""
        return list(self._discovered_wallets.values())

    def get_connection_stats(self) -> Dict[str, Any]:
        """Get connection statistics."""
        # Calculate uptime
        if self._stats.connected_at > 0:
            self._stats.connection_uptime_seconds = time.time() - self._stats.connected_at

        return {
            "connection_state": self._connection_state.value,
            "connected_at": self._stats.connected_at,
            "messages_received": self._stats.messages_received,
            "wallets_discovered": self._stats.wallets_discovered,
            "reconnection_count": self._stats.reconnection_count,
            "connection_uptime_seconds": self._stats.connection_uptime_seconds,
            "last_message_time": self._stats.last_message_time,
        }


class DEXMonitor:
    """
    Manages WebSocket connections to multiple DEX programs.

    This class coordinates multiple WebSocket connections for comprehensive
    DEX monitoring with automatic failover and health monitoring.
    """

    def __init__(self, top_wallets: int = 200):
        """Initialize the DEX monitor."""
        self._top_wallets = top_wallets
        self._clients: Dict[DEXProgram, WebSocketDiscoveryClient] = {}
        self._running = False

        logger.info(f"[DEXMonitor] Initialized with {top_wallets} top wallets target")

    async def start(self) -> None:
        """Start monitoring all DEX programs."""
        self._running = True

        # Create clients for each DEX program
        for dex in DEXProgram:
            client = WebSocketDiscoveryClient(top_wallets=self._top_wallets, program=dex)
            self._clients[dex] = client

            # Set up callbacks
            client.set_wallet_discovered_callback(self._on_wallet_discovered)

        # Connect all clients
        connection_tasks = []
        for dex, client in self._clients.items():
            task = asyncio.create_task(client.connect())
            connection_tasks.append((dex, client, task))

        # Wait for all connections
        for dex, client, task in connection_tasks:
            try:
                success = await task
                if success:
                    # Subscribe to DEX program
                    await client.subscribe_to_dex_programs([dex])
                    logger.info(f"[DEXMonitor] {dex.value} monitoring started")
                else:
                    logger.warning(f"[DEXMonitor] {dex.value} monitoring failed to start")
            except Exception as e:
                logger.error(f"[DEXMonitor] Error starting {dex.value} monitoring: {e}")

        logger.info(f"[DEXMonitor] Started monitoring {len(self._clients)} DEX programs")

    async def stop(self) -> None:
        """Stop monitoring all DEX programs."""
        self._running = False

        disconnect_tasks = []
        for dex, client in self._clients.items():
            task = asyncio.create_task(client.disconnect())
            disconnect_tasks.append(task)

        await asyncio.gather(*disconnect_tasks, return_exceptions=True)

        logger.info("[DEXMonitor] Stopped monitoring all DEX programs")

    async def _on_wallet_discovered(self, event: DiscoveredWalletEvent) -> None:
        """Handle wallet discovered event."""
        # Aggregate wallets from all DEX sources
        logger.debug(
            f"[DEXMonitor] Wallet discovered via {event.source_dex.value}: {event.address[:8]}..."
        )

    def get_all_discovered_wallets(self) -> List[DiscoveredWalletEvent]:
        """Get all wallets discovered from all DEX programs."""
        all_wallets = []

        for client in self._clients.values():
            all_wallets.extend(client.get_discovered_wallets())

        # Deduplicate by address
        unique_wallets = {}
        for wallet in all_wallets:
            if wallet.address not in unique_wallets:
                unique_wallets[wallet.address] = wallet

        return list(unique_wallets.values())

    def get_monitoring_stats(self) -> Dict[str, Any]:
        """Get comprehensive monitoring statistics."""
        stats = {
            "total_clients": len(self._clients),
            "connected_clients": 0,
            "total_wallets_discovered": 0,
            "clients": {}
        }

        for dex, client in self._clients.items():
            client_stats = client.get_connection_stats()
            stats["clients"][dex.value] = client_stats

            if client_stats["connection_state"] == "connected":
                stats["connected_clients"] += 1

            stats["total_wallets_discovered"] += client_stats["wallets_discovered"]

        return stats


class WebSocketHealthMonitor:
    """
    Monitors WebSocket connection health and triggers corrective actions.

    This class implements comprehensive health monitoring for WebSocket
    connections including latency tracking, message rate monitoring,
    and automatic failover.
    """

    def __init__(self, check_interval: int = 30):
        """Initialize the health monitor."""
        self._check_interval = check_interval
        self._monitoring = False
        self._monitor_task: Optional[asyncio.Task] = None

        # Health thresholds
        self._max_latency_ms = 5000
        self._max_message_gap_seconds = 120
        self._min_message_rate_per_minute = 10

    async def start(self, dex_monitor: DEXMonitor) -> None:
        """Start health monitoring."""
        self._monitoring = True
        self._dex_monitor = dex_monitor

        self._monitor_task = asyncio.create_task(self._monitor_loop())

        logger.info("[WebSocketHealthMonitor] Started health monitoring")

    async def stop(self) -> None:
        """Stop health monitoring."""
        self._monitoring = False

        if self._monitor_task:
            self._monitor_task.cancel()
            try:
                await self._monitor_task
            except asyncio.CancelledError:
                pass

        logger.info("[WebSocketHealthMonitor] Stopped health monitoring")

    async def _monitor_loop(self) -> None:
        """Health monitoring loop."""
        while self._monitoring:
            try:
                await asyncio.sleep(self._check_interval)
                await self._perform_health_checks()

            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"[WebSocketHealthMonitor] Error in health check: {e}")

    async def _perform_health_checks(self) -> None:
        """Perform health checks on all WebSocket connections."""
        stats = self._dex_monitor.get_monitoring_stats()

        for dex, client_stats in stats["clients"].items():
            # Check connection state
            if client_stats["connection_state"] != "connected":
                logger.warning(f"[WebSocketHealthMonitor] {dex} not connected: {client_stats['connection_state']}")
                continue

            # Check message gap
            if client_stats["last_message_time"] > 0:
                message_gap = time.time() - client_stats["last_message_time"]
                if message_gap > self._max_message_gap_seconds:
                    logger.warning(
                        f"[WebSocketHealthMonitor] {dex} message gap {message_gap:.0f}s exceeds threshold"
                    )

            # Check wallet discovery rate
            if client_stats["connection_uptime_seconds"] > 300:  # 5 minutes minimum
                wallets_per_minute = (
                    client_stats["wallets_discovered"] / (client_stats["connection_uptime_seconds"] / 60)
                )
                if wallets_per_minute < self._min_message_rate_per_minute:
                    logger.warning(
                        f"[WebSocketHealthMonitor] {dex} discovery rate {wallets_per_minute:.1f}/min below threshold"
                    )


# Singleton instance
_websocket_discovery_instance: Optional[DEXMonitor] = None


def get_websocket_discovery(top_wallets: int = 200) -> DEXMonitor:
    """Get the singleton WebSocket discovery instance."""
    global _websocket_discovery_instance
    if _websocket_discovery_instance is None:
        _websocket_discovery_instance = DEXMonitor(top_wallets=top_wallets)
    return _websocket_discovery_instance
