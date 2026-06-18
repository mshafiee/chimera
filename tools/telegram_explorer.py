#!/usr/bin/env python3
"""
Telegram Signal Explorer for Chimera

This script reads and analyzes trading signals from Telegram channels to determine
their value for integration into Chimera's copy-trading system.

Features:
- Reads messages from specified Telegram channels
- Parses different signal formats (text, images, links)
- Extracts token addresses, entry prices, targets
- Calculates ROI performance for tracked signals
- Generates analysis report with channel rankings

Usage:
    python telegram_explorer.py --config telegram_config.yaml

Requirements:
    - telethon>=0.30
    - python-telegram-bot>=21.0
    - aiohttp>=3.9
    - pandas>=2.0
    - pyyaml>=6.0
"""

import asyncio
import json
import re
import logging
import argparse
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Dict, List, Optional, Any, Tuple
from dataclasses import dataclass, asdict
from collections import defaultdict
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


# Regex patterns for Solana token addresses (base58, 32-44 chars)
SOLANA_ADDRESS_PATTERN = re.compile(
    r'\b[1-9A-HJ-NP-Za-km-z]{32,44}\b'
)

# Common patterns indicating trading signals
SIGNAL_KEYWORDS = [
    'pump', 'buy', 'entry', 'target', 'launch', 'new token', 'gem',
    'call', 'moon', 'rocket', 'alpha', 'contract', 'address',
    'ca:', 'contract:', 'token:', 'symbol:', 'ticker:', '$'
]

# Emoji indicators for confidence level
HIGH_CONFIDENCE_EMOJIS = ['🚀', '💎', '🔥', '⭐', '✅', '💯']
MEDIUM_CONFIDENCE_EMOJIS = ['👀', '📈', '🔔', '⚡', '💰']
LOW_CONFIDENCE_EMOJIS = ['⚠️', '🤡', '📉', '❌', '💀']


@dataclass
class TelegramSignal:
    """Represents a parsed trading signal from Telegram."""
    channel: str
    message_id: int
    timestamp: datetime
    signal_type: str  # 'pump', 'call', 'launch', 'unknown'
    token_address: Optional[str] = None
    token_symbol: Optional[str] = None
    entry_price: Optional[float] = None
    target_price: Optional[float] = None
    liquidity: Optional[float] = None
    confidence: str = 'unknown'  # 'low', 'medium', 'high'
    has_chart: bool = False
    has_caution: bool = False
    raw_text: str = ''
    links: List[str] = None

    def __post_init__(self):
        if self.links is None:
            self.links = []

    def to_dict(self) -> Dict:
        """Convert to dictionary for JSON serialization."""
        data = asdict(self)
        data['timestamp'] = self.timestamp.isoformat()
        return data


@dataclass
class ChannelMetrics:
    """Metrics for a single Telegram channel."""
    channel: str
    message_count: int = 0
    signal_count: int = 0
    parse_success_rate: float = 0.0
    signals_per_day: float = 0.0
    avg_confidence: str = 'unknown'
    signal_types: Dict[str, int] = None
    parse_errors: int = 0

    # Performance metrics (if tracking enabled)
    tracked_signals: int = 0
    roi_1h: Optional[float] = None
    roi_24h: Optional[float] = None
    roi_7d: Optional[float] = None
    win_rate: Optional[float] = None

    def __post_init__(self):
        if self.signal_types is None:
            self.signal_types = defaultdict(int)

    def to_dict(self) -> Dict:
        """Convert to dictionary for JSON serialization."""
        return {
            'channel': self.channel,
            'message_count': self.message_count,
            'signal_count': self.signal_count,
            'parse_success_rate': self.parse_success_rate,
            'signals_per_day': self.signals_per_day,
            'avg_confidence': self.avg_confidence,
            'signal_types': dict(self.signal_types),
            'parse_errors': self.parse_errors,
            'performance': {
                'tracked_signals': self.tracked_signals,
                'roi_1h': self.roi_1h,
                'roi_24h': self.roi_24h,
                'roi_7d': self.roi_7d,
                'win_rate': self.win_rate
            }
        }


class SignalParser:
    """Parser for extracting trading signals from Telegram messages."""

    @staticmethod
    def is_signal_message(text: str) -> bool:
        """Check if a message contains trading signal indicators."""
        text_lower = text.lower()
        return any(keyword in text_lower for keyword in SIGNAL_KEYWORDS)

    @staticmethod
    def extract_token_address(text: str) -> Optional[str]:
        """Extract Solana token address from text."""
        # Look for patterns like "ca:", "contract:", "address:" followed by address
        for prefix in ['ca:', 'contract:', 'address:', 'token:']:
            pattern = re.compile(re.escape(prefix) + r'\s*([1-9A-HJ-NP-Za-km-z]{32,44})', re.IGNORECASE)
            match = pattern.search(text)
            if match:
                return match.group(1)

        # Fallback: find first valid Solana address
        matches = SOLANA_ADDRESS_PATTERN.findall(text)
        if matches:
            return matches[0]

        return None

    @staticmethod
    def extract_token_symbol(text: str) -> Optional[str]:
        """Extract token ticker/symbol from text."""
        # Look for $SYMBOL patterns
        dollar_pattern = re.compile(r'\$([A-Z]{1,10})\b')
        match = dollar_pattern.search(text)
        if match:
            return match.group(1)

        # Look for "symbol:" or "ticker:" patterns
        for prefix in ['symbol:', 'ticker:', 'name:']:
            pattern = re.compile(re.escape(prefix) + r'\s*([A-Z]{1,10})\b', re.IGNORECASE)
            match = pattern.search(text)
            if match:
                return match.group(1)

        return None

    @staticmethod
    def extract_price(text: str) -> Tuple[Optional[float], Optional[float]]:
        """Extract entry and target prices from text."""
        entry_price = None
        target_price = None

        # Look for "entry:" or "buy:" patterns
        entry_pattern = re.compile(
            r'(?:entry|buy|entry\s*price)[:\s]*\$?(\d+(?:\.\d+)?)',
            re.IGNORECASE
        )
        entry_match = entry_pattern.search(text)
        if entry_match:
            entry_price = float(entry_match.group(1))

        # Look for "target:" or "tp:" patterns
        target_pattern = re.compile(
            r'(?:target|tp|take\s*profit|goal)[:\s]*\$?(\d+(?:\.\d+)?)',
            re.IGNORECASE
        )
        target_match = target_pattern.search(text)
        if target_match:
            target_price = float(target_match.group(1))

        return entry_price, target_price

    @staticmethod
    def extract_liquidity(text: str) -> Optional[float]:
        """Extract liquidity value from text."""
        # Look for patterns like "liquidity: $10k" or "$50k liq"
        liq_pattern = re.compile(
            r'(?:liquidity|liq|pool)[:\s]*\$?(\d+(?:\.\d+)?)\s*(k|m)?',
            re.IGNORECASE
        )
        match = liq_pattern.search(text)
        if match:
            value = float(match.group(1))
            multiplier = match.group(2)
            if multiplier:
                if multiplier.lower() == 'k':
                    value *= 1000
                elif multiplier.lower() == 'm':
                    value *= 1_000_000
            return value

        return None

    @staticmethod
    def determine_confidence(text: str) -> str:
        """Determine confidence level based on emojis and text."""
        text_lower = text.lower()

        # Check for explicit confidence indicators
        if any(word in text_lower for word in ['strong buy', 'high confidence', 'sure']):
            return 'high'
        if any(word in text_lower for word in ['speculative', 'risky', 'low confidence']):
            return 'low'

        # Check emoji indicators
        high_count = sum(1 for emoji in HIGH_CONFIDENCE_EMOJIS if emoji in text)
        medium_count = sum(1 for emoji in MEDIUM_CONFIDENCE_EMOJIS if emoji in text)
        low_count = sum(1 for emoji in LOW_CONFIDENCE_EMOJIS if emoji in text)

        if high_count >= 2 or (high_count >= 1 and low_count == 0):
            return 'high'
        elif low_count >= 2:
            return 'low'
        elif medium_count >= 1 or high_count == 1:
            return 'medium'

        return 'medium'

    @staticmethod
    def determine_signal_type(text: str) -> str:
        """Determine the type of signal."""
        text_lower = text.lower()

        if 'launch' in text_lower or 'new token' in text_lower or 'just launched' in text_lower:
            return 'launch'
        elif 'call' in text_lower or '🎯' in text:
            return 'call'
        elif 'pump' in text_lower or 'pumping' in text_lower:
            return 'pump'
        else:
            return 'unknown'

    @staticmethod
    def extract_links(message: Any) -> List[str]:
        """Extract links from a Telegram message."""
        links = []
        if hasattr(message, 'entities'):
            for entity in message.entities:
                if isinstance(entity, types.MessageEntityUrl):
                    links.append(entity.url)
        return links

    @staticmethod
    def has_chart_link(links: List[str]) -> bool:
        """Check if any link is a charting website."""
        chart_domains = ['dexscreener.com', 'dexscreener', 'photon.sol',
                        'birdeye.so', 'birdeye', 'bullx', 'geckoterminal']
        return any(domain in link.lower() for link in links for domain in chart_domains)

    @staticmethod
    def has_caution(text: str) -> bool:
        """Check if message includes NFA/disclaimer."""
        caution_keywords = ['nfa', 'not financial advice', 'do your own research',
                           'dyor', 'risk', 'not advice']
        text_lower = text.lower()
        return any(keyword in text_lower for keyword in caution_keywords)

    def parse_message(self, message: Any, channel: str) -> Optional[TelegramSignal]:
        """Parse a Telegram message into a signal if it contains trading information."""
        if not message.text:
            return None

        text = message.text

        # Check if this looks like a signal
        if not self.is_signal_message(text):
            return None

        # Extract signal components
        token_address = self.extract_token_address(text)
        if not token_address:
            return None  # Must have token address to be a valid signal

        token_symbol = self.extract_token_symbol(text)
        entry_price, target_price = self.extract_price(text)
        liquidity = self.extract_liquidity(text)
        confidence = self.determine_confidence(text)
        signal_type = self.determine_signal_type(text)
        links = self.extract_links(message)
        has_chart = self.has_chart_link(links)
        has_caution = self.has_caution(text)

        return TelegramSignal(
            channel=channel,
            message_id=message.id,
            timestamp=message.date,
            signal_type=signal_type,
            token_address=token_address,
            token_symbol=token_symbol,
            entry_price=entry_price,
            target_price=target_price,
            liquidity=liquidity,
            confidence=confidence,
            has_chart=has_chart,
            has_caution=has_caution,
            raw_text=text[:500],  # Truncate for storage
            links=links
        )


class PerformanceTracker:
    """Track and calculate ROI performance for signals."""

    def __init__(self, rpc_url: str, min_volume_threshold: float = 10000):
        """
        Initialize performance tracker.

        Args:
            rpc_url: Helius/Solana RPC URL for price data
            min_volume_threshold: Minimum volume to track (USD)
        """
        self.rpc_url = rpc_url
        self.min_volume_threshold = min_volume_threshold
        self.session: Optional[aiohttp.ClientSession] = None

    async def __aenter__(self):
        self.session = aiohttp.ClientSession()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        if self.session:
            await self.session.close()

    async def get_token_price(self, token_address: str) -> Optional[float]:
        """Get current price of a token via Helius API."""
        if not self.session:
            return None

        try:
            # Use Helius API for token price
            url = f"{self.rpc_url.rstrip('/')}"
            payload = {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getAssetPrice",
                "params": {
                    "id": token_address
                }
            }

            async with self.session.post(url, json=payload, timeout=10) as response:
                if response.status == 200:
                    data = await response.json()
                    if 'result' in data:
                        price_info = data['result']
                        if isinstance(price_info, dict) and 'price' in price_info:
                            return float(price_info['price'])

        except Exception as e:
            logger.debug(f"Error fetching price for {token_address}: {e}")

        return None

    async def calculate_signal_roi(
        self,
        signal: TelegramSignal,
        check_intervals: List[timedelta]
    ) -> Dict[str, Optional[float]]:
        """
        Calculate ROI for a signal at different time intervals.

        Note: This requires historical price data which may not be readily available.
        This is a placeholder for the actual implementation.
        """
        # TODO: Implement with historical price API
        # For now, return placeholder data
        return {
            '1h': None,
            '24h': None,
            '7d': None
        }


class TelegramExplorer:
    """Main explorer class for analyzing Telegram channels."""

    def __init__(self, config_path: str):
        """Initialize the explorer with configuration."""
        self.config = self._load_config(config_path)
        self.client: Optional[TelegramClient] = None
        self.parser = SignalParser()
        self.signals: List[TelegramSignal] = []
        self.channel_metrics: Dict[str, ChannelMetrics] = {}

        # Initialize performance tracker if enabled
        self.performance_enabled = self.config.get('performance_tracking', {}).get('enabled', False)
        self.performance_tracker: Optional[PerformanceTracker] = None

        if self.performance_enabled:
            rpc_url = self.config.get('rpc_url') or os.environ.get('CHIMERA_RPC__PRIMARY_URL')
            if rpc_url:
                self.performance_tracker = PerformanceTracker(rpc_url)

    def _load_config(self, config_path: str) -> Dict:
        """Load configuration from YAML file."""
        config_file = Path(config_path)
        if not config_file.exists():
            raise FileNotFoundError(f"Config file not found: {config_path}")

        with open(config_file, 'r') as f:
            config = yaml.safe_load(f)

        return config

    async def start(self):
        """Start the Telegram client."""
        telegram_config = self.config.get('telegram', {})

        api_id = os.environ.get('TELEGRAM_API_ID') or telegram_config.get('api_id')
        api_hash = os.environ.get('TELEGRAM_API_HASH') or telegram_config.get('api_hash')
        session_name = telegram_config.get('session_name', 'telegram_explorer')

        if not api_id or not api_hash:
            raise ValueError(
                "Telegram API credentials not found. "
                "Set TELEGRAM_API_ID and TELEGRAM_API_HASH environment variables "
                "or add them to telegram_config.yaml"
            )

        self.client = TelegramClient(session_name, api_id, api_hash)
        await self.client.start()

        logger.info("Telegram client started successfully")

    async def stop(self):
        """Stop the Telegram client and cleanup."""
        if self.client:
            await self.client.disconnect()
            logger.info("Telegram client disconnected")

        if self.performance_tracker:
            await self.performance_tracker.__aexit__(None, None, None)

    async def explore_channel(self, channel_username: str) -> ChannelMetrics:
        """Explore a single Telegram channel for signals."""
        logger.info(f"Exploring channel: {channel_username}")

        exploration_config = self.config.get('exploration', {})
        days_back = exploration_config.get('days_back', 7)
        message_limit = exploration_config.get('message_limit', 200)

        metrics = ChannelMetrics(channel=channel_username)
        signals_found = []

        try:
            # Resolve channel entity
            entity = await self.client.get_entity(channel_username)

            # Calculate date threshold
            date_threshold = datetime.now(timezone.utc) - timedelta(days=days_back)

            # Fetch messages
            async for message in self.client.iter_messages(
                entity,
                limit=message_limit,
                offset_date=date_threshold
            ):
                metrics.message_count += 1

                if message.text:
                    # Try to parse as a signal
                    try:
                        signal = self.parser.parse_message(message, channel_username)
                        if signal:
                            signals_found.append(signal)
                            metrics.signal_count += 1
                            metrics.signal_types[signal.signal_type] += 1

                            # Track confidence distribution
                            if metrics.avg_confidence == 'unknown':
                                metrics.avg_confidence = signal.confidence

                    except Exception as e:
                        logger.debug(f"Error parsing message {message.id}: {e}")
                        metrics.parse_errors += 1

        except ChannelPrivateError:
            logger.error(f"Cannot access private channel: {channel_username}")
            metrics.parse_errors = message_limit
        except Exception as e:
            logger.error(f"Error exploring {channel_username}: {e}")
            metrics.parse_errors += 1

        # Calculate derived metrics
        if metrics.message_count > 0:
            metrics.parse_success_rate = metrics.signal_count / metrics.message_count
            metrics.signals_per_day = metrics.signal_count / days_back

        # Store signals for this channel
        self.signals.extend(signals_found)
        self.channel_metrics[channel_username] = metrics

        logger.info(
            f"Channel {channel_username}: {metrics.signal_count} signals found "
            f"({metrics.parse_success_rate:.1%} success rate)"
        )

        return metrics

    async def explore_all_channels(self) -> Dict[str, ChannelMetrics]:
        """Explore all configured channels."""
        channels = self.config.get('channels', [])

        if not channels:
            raise ValueError("No channels configured in telegram_config.yaml")

        logger.info(f"Exploring {len(channels)} channels...")

        # Explore channels concurrently
        tasks = [self.explore_channel(channel) for channel in channels]
        await asyncio.gather(*tasks, return_exceptions=True)

        return self.channel_metrics

    def calculate_channel_scores(self) -> Dict[str, float]:
        """Calculate value scores for each channel."""
        scores = {}

        for channel, metrics in self.channel_metrics.items():
            score = 0.0

            # Signal Frequency (20% weight)
            # >= 2 signals/day = full points
            frequency_score = min(metrics.signals_per_day / 2.0, 1.0) * 20
            score += frequency_score

            # Parseability (25% weight)
            # >= 80% parse success = full points
            parse_score = min(metrics.parse_success_rate / 0.8, 1.0) * 25
            score += parse_score

            # Completeness (20% weight)
            # Estimate based on avg confidence
            confidence_scores = {'high': 1.0, 'medium': 0.6, 'low': 0.3, 'unknown': 0.5}
            completeness_score = confidence_scores.get(metrics.avg_confidence, 0.5) * 20
            score += completeness_score

            # Consistency (15% weight)
            # If signals_per_day > 0.5, consider consistent
            consistency_score = min(metrics.signals_per_day / 0.5, 1.0) * 15
            if consistency_score < 0:
                consistency_score = 0
            score += consistency_score

            # Performance (20% weight) - if available
            if metrics.win_rate is not None:
                performance_score = (metrics.win_rate * 20)
                score += performance_score

            scores[channel] = round(score, 1)

        return scores

    def generate_report(self) -> Dict:
        """Generate comprehensive analysis report."""
        scores = self.calculate_channel_scores()

        # Classify channels
        high_value = [ch for ch, sc in scores.items() if sc >= 70]
        medium_value = [ch for ch, sc in scores.items() if 40 <= sc < 70]
        low_value = [ch for ch, sc in scores.items() if sc < 40]

        report = {
            'analysis_date': datetime.now(timezone.utc).isoformat(),
            'channels_analyzed': len(self.channel_metrics),
            'days_analyzed': self.config.get('exploration', {}).get('days_back', 7),
            'total_signals': sum(m.signal_count for m in self.channel_metrics.values()),
            'classification': {
                'high_value': high_value,
                'medium_value': medium_value,
                'low_value': low_value
            },
            'channels': {}
        }

        # Add detailed metrics for each channel
        for channel, metrics in self.channel_metrics.items():
            report['channels'][channel] = {
                'metrics': metrics.to_dict(),
                'score': scores.get(channel, 0),
                'sample_signals': [
                    s.to_dict() for s in self.signals[:3]
                    if s.channel == channel
                ]
            }

        return report

    def save_results(self, output_dir: str):
        """Save analysis results to files."""
        output_path = Path(output_dir)
        output_path.mkdir(parents=True, exist_ok=True)

        # Save full report
        report = self.generate_report()
        report_file = output_path / 'summary.json'
        with open(report_file, 'w') as f:
            json.dump(report, f, indent=2, default=str)

        # Save raw signals
        signals_file = output_path / 'raw_signals.json'
        with open(signals_file, 'w') as f:
            json.dump([s.to_dict() for s in self.signals], f, indent=2, default=str)

        # Save channel rankings
        scores = self.calculate_channel_scores()
        rankings_file = output_path / 'rankings.json'
        with open(rankings_file, 'w') as f:
            json.dump(
                sorted(scores.items(), key=lambda x: x[1], reverse=True),
                f,
                indent=2
            )

        logger.info(f"Results saved to {output_dir}")

        # Print summary
        self.print_summary(report, scores)

    def print_summary(self, report: Dict, scores: Dict[str, float]):
        """Print analysis summary to console."""
        print("\n" + "=" * 60)
        print("TELEGRAM CHANNEL ANALYSIS SUMMARY")
        print("=" * 60)

        print(f"\nChannels Analyzed: {report['channels_analyzed']}")
        print(f"Analysis Period: {report['days_analyzed']} days")
        print(f"Total Signals Found: {report['total_signals']}")

        print("\n" + "-" * 60)
        print("CHANNEL RANKINGS")
        print("-" * 60)

        sorted_channels = sorted(scores.items(), key=lambda x: x[1], reverse=True)
        for i, (channel, score) in enumerate(sorted_channels, 1):
            metrics = self.channel_metrics[channel]
            classification = "🟢" if score >= 70 else "🟡" if score >= 40 else "🔴"
            print(f"{i:2d}. {classification} {channel:30s} - Score: {score:5.1f} | "
                  f"Signals: {metrics.signal_count:3d} | "
                  f"Rate: {metrics.signals_per_day:.1f}/day")

        print("\n" + "-" * 60)
        print("CLASSIFICATION")
        print("-" * 60)

        high = report['classification']['high_value']
        medium = report['classification']['medium_value']
        low = report['classification']['low_value']

        print(f"🟢 HIGH VALUE (Score ≥70): {len(high)} channels")
        for ch in high:
            print(f"   - {ch} ({scores[ch]:.1f})")

        print(f"\n🟡 MEDIUM VALUE (Score 40-69): {len(medium)} channels")
        for ch in medium[:5]:  # Show first 5
            print(f"   - {ch} ({scores[ch]:.1f})")
        if len(medium) > 5:
            print(f"   ... and {len(medium) - 5} more")

        print(f"\n🔴 LOW VALUE (Score <40): {len(low)} channels")
        for ch in low[:3]:  # Show first 3
            print(f"   - {ch} ({scores[ch]:.1f})")
        if len(low) > 3:
            print(f"   ... and {len(low) - 3} more")

        print("\n" + "=" * 60)
        print(f"Results saved to: {self.config.get('exploration', {}).get('output_dir', 'tools/telegram_analysis')}")
        print("=" * 60 + "\n")


async def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description='Explore Telegram channels for trading signals')
    parser.add_argument(
        '--config',
        default='tools/telegram_config.yaml',
        help='Path to configuration file'
    )
    args = parser.parse_args()

    explorer = TelegramExplorer(args.config)

    try:
        await explorer.start()
        await explorer.explore_all_channels()
        explorer.save_results(
            explorer.config.get('exploration', {}).get('output_dir', 'tools/telegram_analysis')
        )
    finally:
        await explorer.stop()


if __name__ == '__main__':
    import os
    asyncio.run(main())
