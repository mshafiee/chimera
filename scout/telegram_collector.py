#!/usr/bin/env python3
"""
Telegram Signal Collector Service for Chimera

This service runs in the background, monitoring configured Telegram channels
for trading signals and forwarding them to the Chimera Operator via internal API.

Usage:
    python telegram_collector.py [--config telegram_config.yaml]

Features:
- Monitors multiple Telegram channels concurrently
- Parses trading signals from messages
- Forwards signals to Chimera Operator API
- Tracks channel performance metrics
- Handles rate limiting and deduplication
"""

import asyncio
import json
import logging
import os
import argparse
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Dict, List, Optional, Any
from dataclasses import dataclass, asdict
import aiohttp
import yaml

# Try to import telethon, provide helpful error if missing
try:
    from telethon import TelegramClient, types
    from telethon.errors import SessionPasswordNeededError, ChannelPrivateError
except ImportError:
    print("ERROR: telethon is required. Install with: pip install telethon")
    exit(1)

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


@dataclass
class CollectorConfig:
    """Configuration for the Telegram collector."""
    operator_url: str
    operator_api_key: str
    channels: List[str]
    signal_timeout: int = 300  # seconds
    batch_size: int = 10
    polling_interval: int = 5  # seconds


@dataclass
class TelegramSignal:
    """Represents a trading signal from Telegram."""
    channel: str
    channel_id: int
    message_id: int
    timestamp: datetime
    text: str
    token_address: Optional[str] = None
    token_symbol: Optional[str] = None
    confidence: str = 'unknown'
    has_chart: bool = False
    has_caution: bool = False

    def to_dict(self) -> Dict:
        """Convert to dictionary for JSON serialization."""
        data = asdict(self)
        data['timestamp'] = self.timestamp.isoformat()
        return data


class SignalParser:
    """Parser for extracting trading signals from Telegram messages."""

    # Solana token address pattern (base58, 32-44 chars)
    TOKEN_ADDRESS_PATTERN = r'\b[1-9A-HJ-NP-Za-km-z]{32,44}\b'

    # Keywords indicating trading signals
    SIGNAL_KEYWORDS = [
        'pump', 'buy', 'entry', 'target', 'launch', 'new token', 'gem',
        'call', 'moon', 'rocket', 'alpha', 'contract', 'address',
        'ca:', 'contract:', 'token:', 'symbol:', 'ticker:', '$'
    ]

    # Emoji indicators
    HIGH_CONFIDENCE_EMOJIS = ['🚀', '💎', '🔥', '⭐', '✅', '💯']
    MEDIUM_CONFIDENCE_EMOJIS = ['👀', '📈', '🔔', '⚡', '💰']
    LOW_CONFIDENCE_EMOJIS = ['⚠️', '🤡', '📉', '❌', '💀']

    def __init__(self):
        import re
        self.token_pattern = re.compile(self.TOKEN_ADDRESS_PATTERN)
        self.symbol_pattern = re.compile(r'\$([A-Z]{1,10})\b')

    def is_signal_message(self, text: str) -> bool:
        """Check if message contains trading signal indicators."""
        text_lower = text.lower()
        return any(keyword in text_lower for keyword in self.SIGNAL_KEYWORDS)

    def extract_token_address(self, text: str) -> Optional[str]:
        """Extract Solana token address from text."""
        # Look for patterns like "ca:", "contract:", "address:"
        for prefix in ['ca:', 'contract:', 'address:', 'token:']:
            pattern_str = f'{prefix}\\s*([1-9A-HJ-NP-Za-km-z]{{32,44}})'
            pattern = re.compile(pattern_str, re.IGNORECASE)
            match = pattern.search(text)
            if match:
                return match.group(1)

        # Fallback: find first valid Solana address
        matches = self.token_pattern.findall(text)
        if matches:
            return matches[0]

        return None

    def extract_token_symbol(self, text: str) -> Optional[str]:
        """Extract token ticker/symbol from text."""
        # Look for $SYMBOL patterns
        match = self.symbol_pattern.search(text)
        if match:
            return match.group(1)

        # Look for "symbol:" or "ticker:" patterns
        for prefix in ['symbol:', 'ticker:', 'name:']:
            pattern_str = f'{prefix}\\s*([A-Z]{{1,10}})\\b'
            pattern = re.compile(pattern_str, re.IGNORECASE)
            match = pattern.search(text)
            if match:
                return match.group(1)

        return None

    def determine_confidence(self, text: str) -> str:
        """Determine confidence level based on emojis and text."""
        text_lower = text.lower()

        # Check for explicit confidence indicators
        if any(word in text_lower for word in ['strong buy', 'high confidence', 'sure']):
            return 'high'
        if any(word in text_lower for word in ['speculative', 'risky', 'low confidence']):
            return 'low'

        # Check emoji indicators
        high_count = sum(1 for emoji in self.HIGH_CONFIDENCE_EMOJIS if emoji in text)
        medium_count = sum(1 for emoji in self.MEDIUM_CONFIDENCE_EMOJIS if emoji in text)
        low_count = sum(1 for emoji in self.LOW_CONFIDENCE_EMOJIS if emoji in text)

        if high_count >= 2 or (high_count >= 1 and low_count == 0):
            return 'high'
        elif low_count >= 2:
            return 'low'
        elif medium_count >= 1 or high_count == 1:
            return 'medium'

        return 'medium'

    def has_chart_link(self, text: str) -> bool:
        """Check if message includes chart link."""
        chart_domains = [
            'dexscreener.com', 'dexscreener',
            'photon.sol', 'birdeye.so', 'birdeye',
            'bullx', 'geckoterminal'
        ]
        return any(domain in text.lower() for domain in chart_domains)

    def has_caution(self, text: str) -> bool:
        """Check if message includes NFA/disclaimer."""
        caution_keywords = [
            'nfa', 'not financial advice', 'do your own research',
            'dyor', 'risk', 'not advice'
        ]
        text_lower = text.lower()
        return any(keyword in text_lower for keyword in caution_keywords)

    def parse_message(self, message, channel: str) -> Optional[TelegramSignal]:
        """Parse a Telegram message into a signal if it contains trading information."""
        if not message.text:
            return None

        text = message.text

        # Check if this looks like a signal
        if not self.is_signal_message(text):
            return None

        # Extract token address (required)
        token_address = self.extract_token_address(text)
        if not token_address:
            return None

        # Extract optional fields
        token_symbol = self.extract_token_symbol(text)
        confidence = self.determine_confidence(text)
        has_chart = self.has_chart_link(text)
        has_caution = self.has_caution(text)

        return TelegramSignal(
            channel=channel,
            channel_id=message.chat_id,
            message_id=message.id,
            timestamp=message.date,
            text=text[:500],  # Truncate for storage
            token_address=token_address,
            token_symbol=token_symbol,
            confidence=confidence,
            has_chart=has_chart,
            has_caution=has_caution
        )


class TelegramCollector:
    """Main collector service for monitoring Telegram channels."""

    def __init__(self, config: CollectorConfig, client: TelegramClient):
        """
        Initialize the collector.

        Args:
            config: Collector configuration
            client: Telethon client instance
        """
        self.config = config
        self.client = client
        self.parser = SignalParser()
        self.session: Optional[aiohttp.ClientSession] = None
        self.processed_messages: Dict[str, set] = {}  # Track processed messages
        self.signal_count = 0
        self.error_count = 0

    async def __aenter__(self):
        """Initialize HTTP session."""
        self.session = aiohttp.ClientSession()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Cleanup HTTP session."""
        if self.session:
            await self.session.close()

    async def send_signal_to_operator(self, signal: TelegramSignal) -> bool:
        """
        Send parsed signal to Chimera Operator.

        Args:
            signal: Parsed Telegram signal

        Returns:
            True if successful, False otherwise
        """
        if not self.session:
            logger.error("HTTP session not initialized")
            return False

        try:
            headers = {
                'Content-Type': 'application/json',
            }

            if self.config.operator_api_key:
                headers['Authorization'] = f'Bearer {self.config.operator_api_key}'

            payload = {
                'channel': signal.channel,
                'channel_id': signal.channel_id,
                'message_id': signal.message_id,
                'timestamp': int(signal.timestamp.timestamp()),
                'text': signal.text,
            }

            async with self.session.post(
                f'{self.config.operator_url}/api/v1/telegram/signal',
                json=payload,
                headers=headers,
                timeout=aiohttp.ClientTimeout(total=10)
            ) as response:
                if response.status == 200:
                    logger.info(f"Signal sent to operator: {signal.channel} -> {signal.token_address}")
                    return True
                else:
                    error_text = await response.text()
                    logger.error(f"Operator rejected signal: {response.status} - {error_text}")
                    return False

        except asyncio.TimeoutError:
            logger.error("Timeout sending signal to operator")
            return False
        except Exception as e:
            logger.error(f"Error sending signal to operator: {e}")
            return False

    async def monitor_channel(self, channel: str):
        """
        Monitor a single Telegram channel for signals.

        Args:
            channel: Channel username (e.g., "@solana_whales_signal")
        """
        logger.info(f"Monitoring channel: {channel}")

        # Initialize processed message tracking for this channel
        if channel not in self.processed_messages:
            self.processed_messages[channel] = set()

        try:
            # Resolve channel entity
            entity = await self.client.get_entity(channel)

            # Fetch recent messages (last hour)
            since = datetime.now(timezone.utc) - timedelta(hours=1)

            async for message in self.client.iter_messages(
                entity,
                limit=self.config.batch_size,
                offset_date=since
            ):
                # Skip already processed messages
                message_key = f"{channel}:{message.id}"
                if message.id in self.processed_messages[channel]:
                    continue

                # Try to parse as a signal
                try:
                    signal = self.parser.parse_message(message, channel)
                    if signal:
                        # Send to operator
                        success = await self.send_signal_to_operator(signal)
                        if success:
                            self.signal_count += 1
                        else:
                            self.error_count += 1

                        # Mark as processed
                        self.processed_messages[channel].add(message.id)

                except Exception as e:
                    logger.debug(f"Error parsing message {message.id}: {e}")
                    self.error_count += 1

                # Mark as processed regardless of parsing result
                self.processed_messages[channel].add(message.id)

        except ChannelPrivateError:
            logger.error(f"Cannot access private channel: {channel}")
        except Exception as e:
            logger.error(f"Error monitoring {channel}: {e}")

    async def monitor_all_channels(self):
        """Monitor all configured channels."""
        logger.info(f"Monitoring {len(self.config.channels)} channels...")

        while True:
            # Monitor all channels concurrently
            tasks = [
                self.monitor_channel(channel)
                for channel in self.config.channels
            ]
            await asyncio.gather(*tasks, return_exceptions=True)

            logger.info(
                f"Monitoring cycle complete. Signals: {self.signal_count}, Errors: {self.error_count}"
            )

            # Wait before next cycle
            await asyncio.sleep(self.config.polling_interval)


async def load_config(config_path: str) -> CollectorConfig:
    """Load configuration from YAML file."""
    config_file = Path(config_path)
    if not config_file.exists():
        raise FileNotFoundError(f"Config file not found: {config_path}")

    with open(config_file, 'r') as f:
        config_data = yaml.safe_load(f)

    # Get operator URL from config or environment
    operator_url = os.environ.get('CHIMERA_OPERATOR_URL', 'http://localhost:8080')
    operator_api_key = os.environ.get('CHIMERA_API_KEY', '')

    # Get channels from telegram config or use defaults
    telegram_config = config_data.get('telegram', {})
    channels_from_config = telegram_config.get('channels', [])

    # Use high-value channels from analysis
    default_channels = [
        '@solana_whales_signal',
        '@SolmemeWhaleinsider',
        '@SolanaDaily_Pumps',
    ]

    channels = channels_from_config if channels_from_config else default_channels

    return CollectorConfig(
        operator_url=operator_url,
        operator_api_key=operator_api_key,
        channels=channels,
        signal_timeout=config_data.get('exploration', {}).get('days_back', 7) * 86400,
        batch_size=config_data.get('exploration', {}).get('message_limit', 200),
        polling_interval=5,
    )


async def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description='Telegram Signal Collector for Chimera')
    parser.add_argument(
        '--config',
        default='tools/telegram_config.yaml',
        help='Path to configuration file'
    )
    args = parser.parse_args()

    try:
        # Load configuration
        config = await load_config(args.config)

        # Get Telegram API credentials
        api_id = os.environ.get('TELEGRAM_API_ID')
        api_hash = os.environ.get('TELEGRAM_API_HASH')
        session_name = 'telegram_collector'

        if not api_id or not api_hash:
            raise ValueError(
                "Telegram API credentials not found. "
                "Set TELEGRAM_API_ID and TELEGRAM_API_HASH environment variables."
            )

        # Create Telegram client
        client = TelegramClient(session_name, int(api_id), api_hash)

        async with client:
            await client.start()

            logger.info("Telegram collector started successfully")
            logger.info(f"Monitoring {len(config.channels)} channels")
            logger.info(f"Operator URL: {config.operator_url}")

            # Start monitoring
            async with TelegramCollector(config, client) as collector:
                await collector.monitor_all_channels()

    except FileNotFoundError as e:
        logger.error(f"Config file error: {e}")
    except ValueError as e:
        logger.error(f"Configuration error: {e}")
    except KeyboardInterrupt:
        logger.info("Shutting down...")
    except Exception as e:
        logger.error(f"Unexpected error: {e}")
        raise


if __name__ == '__main__':
    import asyncio
    import re  # For SignalParser
    asyncio.run(main())
