#!/usr/bin/env python3
"""
signal-replayer.py - Historical signal replay system for Chimera evaluation

This script enables controlled testing using historical trading signals,
allowing the evaluation to replay real-world signal patterns while
capturing comprehensive performance data.

Usage:
    python3 signal-replayer.py \
        --signal-file /opt/chimera/evaluation/signals/historical_signals.jsonl \
        --webhook-url http://localhost:8080/api/v1/webhook \
        --webhook-secret ${CHIMERA_SECURITY__WEBHOOK_SECRET} \
        --replay-speed 10.0
"""

import argparse
import json
import hmac
import hashlib
import time
import requests
from datetime import datetime
from pathlib import Path
from typing import List, Dict, Any, Optional
from dataclasses import dataclass
import sys


@dataclass
class HistoricalSignal:
    """Represents a historical trading signal."""
    timestamp: str
    wallet_address: str
    token_address: str
    action: str
    amount_sol: float
    strategy: str = 'shield'
    metadata: Optional[Dict[str, Any]] = None

    def to_webhook_payload(self) -> Dict[str, Any]:
        """Convert signal to webhook payload format."""
        return {
            'timestamp': self.timestamp,
            'wallet_address': self.wallet_address,
            'token_address': self.token_address,
            'action': self.action,
            'amount_sol': self.amount_sol,
            'strategy': self.strategy,
            'metadata': self.metadata or {}
        }


class SignalReplayer:
    """Replay historical trading signals for controlled evaluation testing."""

    def __init__(
        self,
        signal_file: Path,
        webhook_url: str,
        webhook_secret: str,
        replay_speed: float = 1.0,
        max_signals: Optional[int] = None
    ):
        """Initialize the signal replayer.

        Args:
            signal_file: Path to historical signals file (JSONL format)
            webhook_url: Chimera webhook endpoint URL
            webhook_secret: Webhook secret for HMAC signing
            replay_speed: Speed multiplier for replay (1.0 = original timing, 10.0 = 10x faster)
            max_signals: Maximum number of signals to replay (None = all)
        """
        self.signal_file = Path(signal_file)
        self.webhook_url = webhook_url
        self.webhook_secret = webhook_secret.encode('utf-8')
        self.replay_speed = replay_speed
        self.max_signals = max_signals

        self.signals: List[HistoricalSignal] = []
        self.current_index = 0
        self.stats = {
            'total_signals': 0,
            'successful_replays': 0,
            'failed_replays': 0,
            'total_duration_seconds': 0,
            'avg_response_time_ms': 0
        }

    def load_signals(self) -> bool:
        """Load historical signals from file.

        Returns:
            True if loading succeeded
        """
        try:
            if not self.signal_file.exists():
                print(f"Error: Signal file does not exist: {self.signal_file}")
                return False

            self.signals = []
            with open(self.signal_file, 'r') as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue

                    try:
                        signal_data = json.loads(line)
                        signal = HistoricalSignal(
                            timestamp=signal_data.get('timestamp', signal_data.get('time', '')),
                            wallet_address=signal_data.get('wallet_address', signal_data.get('wallet', '')),
                            token_address=signal_data.get('token_address', signal_data.get('token', '')),
                            action=signal_data.get('action', signal_data.get('type', 'buy')),
                            amount_sol=float(signal_data.get('amount_sol', signal_data.get('amount', 0.1))),
                            strategy=signal_data.get('strategy', 'shield'),
                            metadata=signal_data.get('metadata', {})
                        )
                        self.signals.append(signal)

                    except (json.JSONDecodeError, ValueError) as e:
                        print(f"Warning: Skipping invalid signal line: {e}")
                        continue

            # Sort signals by timestamp
            self.signals.sort(key=lambda s: s.timestamp)

            # Apply max_signals limit if specified
            if self.max_signals and len(self.signals) > self.max_signals:
                self.signals = self.signals[:self.max_signals]

            self.stats['total_signals'] = len(self.signals)
            print(f"Loaded {len(self.signals)} historical signals from {self.signal_file}")
            return True

        except Exception as e:
            print(f"Error loading signals: {e}")
            return False

    def generate_hmac_signature(self, payload: bytes) -> str:
        """Generate HMAC-SHA256 signature for webhook payload.

        Args:
            payload: Raw payload bytes

        Returns:
            Hex-encoded signature
        """
        signature = hmac.new(
            self.webhook_secret,
            payload,
            hashlib.sha256
        ).hexdigest()
        return signature

    def send_webhook(self, signal: HistoricalSignal) -> Dict[str, Any]:
        """Send signal to Chimera webhook endpoint.

        Args:
            signal: Signal to send

        Returns:
            Response data with status and timing
        """
        payload = signal.to_webhook_payload()
        payload_bytes = json.dumps(payload, separators=(',', ':')).encode('utf-8')
        signature = self.generate_hmac_signature(payload_bytes)

        headers = {
            'Content-Type': 'application/json',
            'X-Chimera-Signature': f'sha256={signature}',
            'X-Chimera-Timestamp': str(int(time.time()))
        }

        start_time = time.time()
        try:
            response = requests.post(
                self.webhook_url,
                data=payload_bytes,
                headers=headers,
                timeout=30
            )
            response_time_ms = (time.time() - start_time) * 1000

            return {
                'success': response.status_code in [200, 202],
                'status_code': response.status_code,
                'response_time_ms': response_time_ms,
                'response_text': response.text[:200],  # Truncate for logging
                'signal_uuid': response.headers.get('X-Chimera-Trade-UUID'),
                'timestamp': datetime.now().isoformat()
            }

        except Exception as e:
            response_time_ms = (time.time() - start_time) * 1000
            return {
                'success': False,
                'status_code': 0,
                'response_time_ms': response_time_ms,
                'response_text': str(e),
                'signal_uuid': None,
                'timestamp': datetime.now().isoformat()
            }

    def calculate_replay_delay(self, signal_index: int) -> float:
        """Calculate delay before replaying next signal.

        Args:
            signal_index: Index of the next signal

        Returns:
            Delay in seconds
        """
        if signal_index == 0:
            return 0.0

        try:
            current_signal = self.signals[signal_index]
            previous_signal = self.signals[signal_index - 1]

            # Calculate original time difference
            current_time = datetime.fromisoformat(current_signal.timestamp)
            previous_time = datetime.fromisoformat(previous_signal.timestamp)
            original_delay = (current_time - previous_time).total_seconds()

            # Apply replay speed
            replay_delay = original_delay / self.replay_speed

            # Ensure minimum delay (0.1 seconds) and maximum delay (60 seconds)
            return max(0.1, min(replay_delay, 60.0))

        except (ValueError, IndexError) as e:
            print(f"Warning: Error calculating delay for signal {signal_index}: {e}")
            return 1.0  # Default 1 second delay

    def replay_signals(self) -> bool:
        """Replay all historical signals with timing.

        Returns:
            True if replay completed successfully
        """
        if not self.signals:
            print("No signals to replay")
            return False

        print("=" * 60)
        print("Starting Historical Signal Replay")
        print("=" * 60)
        print(f"Total Signals: {len(self.signals)}")
        print(f"Replay Speed: {self.replay_speed}x")
        print(f"Webhook URL: {self.webhook_url}")
        print(f"Start Time: {datetime.now().isoformat()}")
        print("")

        start_time = time.time()
        successful_replays = 0
        failed_replays = 0
        response_times = []

        for i, signal in enumerate(self.signals):
            # Calculate delay before this signal
            delay = self.calculate_replay_delay(i)
            if delay > 0:
                print(f"Delaying {delay:.2f}s before signal {i+1}/{len(self.signals)}")
                time.sleep(delay)

            # Send signal
            print(f"Replaying signal {i+1}/{len(self.signals)}: {signal.wallet_address} → {signal.token_address}")
            result = self.send_webhook(signal)

            # Update statistics
            if result['success']:
                successful_replays += 1
                print(f"  ✓ Success ({result['response_time_ms']:.1f}ms)")
            else:
                failed_replays += 1
                print(f"  ✗ Failed ({result['status_code']}: {result['response_text']})")

            response_times.append(result['response_time_ms'])

            # Update progress every 10 signals
            if (i + 1) % 10 == 0:
                elapsed = time.time() - start_time
                rate = (i + 1) / elapsed
                remaining = len(self.signals) - (i + 1)
                eta = remaining / rate if rate > 0 else 0
                print(f"  Progress: {i+1}/{len(self.signals)} | Speed: {rate:.2f} signals/sec | ETA: {eta:.0f}s")

        # Calculate final statistics
        total_duration = time.time() - start_time
        avg_response_time = sum(response_times) / len(response_times) if response_times else 0

        self.stats.update({
            'successful_replays': successful_replays,
            'failed_replays': failed_replays,
            'total_duration_seconds': total_duration,
            'avg_response_time_ms': avg_response_time
        })

        # Print summary
        print("")
        print("=" * 60)
        print("Signal Replay Complete")
        print("=" * 60)
        print(f"Duration: {total_duration:.1f} seconds")
        print(f"Success Rate: {successful_replays}/{len(self.signals)} ({successful_replays/len(self.signals)*100:.1f}%)")
        print(f"Average Response Time: {avg_response_time:.1f}ms")
        print(f"Replay Speed: {len(self.signals)/total_duration:.2f} signals/second")
        print("=" * 60)

        return True

    def save_replay_log(self, output_file: Path):
        """Save replay log to file for analysis.

        Args:
            output_file: Path to save replay log
        """
        try:
            log_data = {
                'replay_config': {
                    'signal_file': str(self.signal_file),
                    'webhook_url': self.webhook_url,
                    'replay_speed': self.replay_speed,
                    'max_signals': self.max_signals
                },
                'statistics': self.stats,
                'signals_count': len(self.signals),
                'timestamp': datetime.now().isoformat()
            }

            with open(output_file, 'w') as f:
                json.dump(log_data, f, indent=2)

            print(f"Replay log saved to: {output_file}")

        except Exception as e:
            print(f"Error saving replay log: {e}")

    def validate_signals(self) -> bool:
        """Validate loaded signals for correctness.

        Returns:
            True if all signals are valid
        """
        validation_errors = []

        for i, signal in enumerate(self.signals):
            # Check required fields
            if not signal.wallet_address:
                validation_errors.append(f"Signal {i}: Missing wallet_address")
            if not signal.token_address:
                validation_errors.append(f"Signal {i}: Missing token_address")
            if not signal.action:
                validation_errors.append(f"Signal {i}: Missing action")
            if signal.amount_sol <= 0:
                validation_errors.append(f"Signal {i}: Invalid amount_sol ({signal.amount_sol})")
            if signal.strategy not in ['shield', 'spear']:
                validation_errors.append(f"Signal {i}: Invalid strategy ({signal.strategy})")

        if validation_errors:
            print("Validation errors found:")
            for error in validation_errors:
                print(f"  - {error}")
            return False

        print(f"✓ All {len(self.signals)} signals validated successfully")
        return True


def main():
    """Main entry point for signal replay."""
    parser = argparse.ArgumentParser(
        description='Replay historical trading signals for Chimera evaluation'
    )
    parser.add_argument(
        '--signal-file',
        type=str,
        required=True,
        help='Path to historical signals file (JSONL format)'
    )
    parser.add_argument(
        '--webhook-url',
        type=str,
        default='http://localhost:8080/api/v1/webhook',
        help='Chimera webhook endpoint URL'
    )
    parser.add_argument(
        '--webhook-secret',
        type=str,
        required=True,
        help='Webhook secret for HMAC signing'
    )
    parser.add_argument(
        '--replay-speed',
        type=float,
        default=1.0,
        help='Replay speed multiplier (1.0 = original timing, 10.0 = 10x faster)'
    )
    parser.add_argument(
        '--max-signals',
        type=int,
        default=None,
        help='Maximum number of signals to replay'
    )
    parser.add_argument(
        '--output-log',
        type=str,
        default=None,
        help='Path to save replay log'
    )
    parser.add_argument(
        '--validate-only',
        action='store_true',
        help='Only validate signals without replaying'
    )

    args = parser.parse_args()

    # Create replayer
    replayer = SignalReplayer(
        signal_file=Path(args.signal_file),
        webhook_url=args.webhook_url,
        webhook_secret=args.webhook_secret,
        replay_speed=args.replay_speed,
        max_signals=args.max_signals
    )

    # Load signals
    if not replayer.load_signals():
        sys.exit(1)

    # Validate signals
    if not replayer.validate_signals():
        sys.exit(1)

    # Exit if validation only
    if args.validate_only:
        print("Validation complete - exiting (validate-only mode)")
        sys.exit(0)

    # Replay signals
    if not replayer.replay_signals():
        sys.exit(1)

    # Save replay log if specified
    if args.output_log:
        replayer.save_replay_log(Path(args.output_log))

    sys.exit(0)


if __name__ == '__main__':
    main()