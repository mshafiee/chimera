#!/usr/bin/env python3
"""
signal-collector.py - Real-time signal recording for Chimera evaluation

This script records real-time trading signals during the evaluation period
for later analysis and comparison with historical replay results.

Usage:
    python3 signal-collector.py \
        --output-dir /opt/chimera/evaluation/signals/realtime \
        --webhook-endpoint http://localhost:8080/api/v1/webhook \
        --duration-days 5
"""

import argparse
import json
import time
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, Any, Optional
from dataclasses import dataclass, asdict
from collections import defaultdict
import threading
import queue
import os


@dataclass
class RecordedSignal:
    """Represents a recorded real-time signal."""
    recording_timestamp: str
    signal_timestamp: str
    wallet_address: str
    token_address: str
    action: str
    amount_sol: float
    strategy: str
    source: str  # webhook, api, etc.
    metadata: Optional[Dict[str, Any]] = None
    recording_metadata: Optional[Dict[str, Any]] = None


class SignalCollector:
    """Record real-time trading signals for evaluation analysis."""

    def __init__(
        self,
        output_dir: Path,
        recording_duration_days: int = 5,
        max_file_size_mb: int = 100
    ):
        """Initialize the signal collector.

        Args:
            output_dir: Directory to store recorded signals
            recording_duration_days: How many days to record signals
            max_file_size_mb: Maximum size per signal file before rotation
        """
        self.output_dir = Path(output_dir)
        self.recording_duration_days = recording_duration_days
        self.max_file_size_bytes = max_file_size_mb * 1024 * 1024

        self.signal_queue = queue.Queue()
        self.is_recording = False
        self.stats = {
            'total_signals': 0,
            'signals_by_source': defaultdict(int),
            'signals_by_strategy': defaultdict(int),
            'signals_by_action': defaultdict(int),
            'recording_start_time': None,
            'recording_end_time': None,
            'files_created': 0
        }

        # Create output directory
        self.output_dir.mkdir(parents=True, exist_ok=True)

        # Current output file
        self.current_file = None
        self.current_file_path = None
        self.current_signal_count = 0

    def start_recording(self):
        """Start recording signals."""
        self.is_recording = True
        self.stats['recording_start_time'] = datetime.now().isoformat()

        # Create initial output file
        self._rotate_output_file()

        print(f"Signal recording started")
        print(f"Output directory: {self.output_dir}")
        print(f"Duration: {self.recording_duration_days} days")
        print(f"Max file size: {self.max_file_size_bytes / (1024*1024):.1f} MB")

    def stop_recording(self):
        """Stop recording signals."""
        self.is_recording = False
        self.stats['recording_end_time'] = datetime.now().isoformat()

        # Close current file
        if self.current_file:
            self.current_file.close()
            self.current_file = None

        print(f"Signal recording stopped")
        print(f"Total signals recorded: {self.stats['total_signals']}")

    def record_signal(self, signal_data: Dict[str, Any], source: str = 'webhook'):
        """Record a trading signal.

        Args:
            signal_data: Signal data from webhook/API
            source: Source of the signal
        """
        if not self.is_recording:
            return

        # Create recorded signal
        recorded_signal = RecordedSignal(
            recording_timestamp=datetime.now().isoformat(),
            signal_timestamp=signal_data.get('timestamp', datetime.now().isoformat()),
            wallet_address=signal_data.get('wallet_address', ''),
            token_address=signal_data.get('token_address', ''),
            action=signal_data.get('action', 'buy'),
            amount_sol=float(signal_data.get('amount_sol', 0.1)),
            strategy=signal_data.get('strategy', 'shield'),
            source=source,
            metadata=signal_data.get('metadata', {}),
            recording_metadata={
                'collector_version': '1.0',
                'recording_day': self._get_recording_day(),
                'recording_hour': datetime.now().hour
            }
        )

        # Add to queue for async processing
        self.signal_queue.put(recorded_signal)

        # Update statistics
        self.stats['total_signals'] += 1
        self.stats['signals_by_source'][source] += 1
        self.stats['signals_by_strategy'][recorded_signal.strategy] += 1
        self.stats['signals_by_action'][recorded_signal.action] += 1

    def process_signals(self):
        """Process signals from queue and write to file."""
        while self.is_recording or not self.signal_queue.empty():
            try:
                # Get signal from queue with timeout
                signal = self.signal_queue.get(timeout=1.0)

                # Check if we need to rotate file
                if self._should_rotate_file():
                    self._rotate_output_file()

                # Write signal to current file
                if self.current_file:
                    self.current_file.write(json.dumps(asdict(signal)) + '\n')
                    self.current_file.flush()
                    self.current_signal_count += 1

                self.signal_queue.task_done()

            except queue.Empty:
                continue
            except Exception as e:
                print(f"Error processing signal: {e}")
                continue

    def _get_recording_day(self) -> int:
        """Get current day number of recording."""
        if self.stats['recording_start_time']:
            start_time = datetime.fromisoformat(self.stats['recording_start_time'])
            elapsed = datetime.now() - start_time
            return elapsed.days + 1
        return 1

    def _should_rotate_file(self) -> bool:
        """Check if output file should be rotated."""
        if not self.current_file_path:
            return True

        # Check file size
        try:
            file_size = self.current_file_path.stat().st_size
            if file_size >= self.max_file_size_bytes:
                return True
        except FileNotFoundError:
            return True

        return False

    def _rotate_output_file(self):
        """Rotate to new output file."""
        # Close current file if open
        if self.current_file:
            self.current_file.close()

        # Create new file with timestamp
        timestamp = datetime.now().strftime('%Y%m%d_%H%M%S')
        day_num = self._get_recording_day()
        filename = f'realtime-signals-day{day_num}-{timestamp}.jsonl'
        self.current_file_path = self.output_dir / filename

        # Open new file
        self.current_file = open(self.current_file_path, 'a')
        self.current_signal_count = 0
        self.stats['files_created'] += 1

        print(f"Rotated to new signal file: {filename}")

    def get_statistics(self) -> Dict[str, Any]:
        """Get current recording statistics."""
        return {
            'total_signals': self.stats['total_signals'],
            'signals_by_source': dict(self.stats['signals_by_source']),
            'signals_by_strategy': dict(self.stats['signals_by_strategy']),
            'signals_by_action': dict(self.stats['signals_by_action']),
            'recording_start_time': self.stats['recording_start_time'],
            'recording_end_time': self.stats['recording_end_time'],
            'files_created': self.stats['files_created'],
            'current_recording_day': self._get_recording_day(),
            'is_recording': self.is_recording
        }

    def save_summary(self):
        """Save recording summary to file."""
        summary_path = self.output_dir / 'recording-summary.json'
        summary_data = self.get_statistics()

        with open(summary_path, 'w') as f:
            json.dump(summary_data, f, indent=2)

        print(f"Recording summary saved to: {summary_path}")


class WebhookInterceptor:
    """Intercept and record webhook signals."""

    def __init__(self, signal_collector: SignalCollector, port: int = 8090):
        """Initialize webhook interceptor.

        Args:
            signal_collector: Signal collector to record intercepted signals
            port: Port to listen on for webhook interception
        """
        self.signal_collector = signal_collector
        self.port = port
        self.server = None

    def start(self):
        """Start webhook interceptor server."""
        try:
            from http.server import HTTPServer, BaseHTTPRequestHandler

            class WebhookHandler(BaseHTTPRequestHandler):
                def __init__(self, request, client_address, server):
                    self.collector = server.collector
                    super().__init__(request, client_address, server)

                def do_POST(self):
                    # Get content length
                    content_length = int(self.headers.get('Content-Length', 0))

                    # Read request body
                    post_data = self.rfile.read(content_length)

                    try:
                        # Parse signal data
                        signal_data = json.loads(post_data.decode('utf-8'))

                        # Record signal
                        self.collector.record_signal(signal_data, source='webhook')

                        # Send success response
                        self.send_response(200)
                        self.send_header('Content-Type', 'application/json')
                        self.end_headers()
                        response = {'status': 'recorded', 'timestamp': datetime.now().isoformat()}
                        self.wfile.write(json.dumps(response).encode('utf-8'))

                    except (json.JSONDecodeError, ValueError) as e:
                        self.send_response(400)
                        self.send_header('Content-Type', 'application/json')
                        self.end_headers()
                        error_response = {'error': f'Invalid JSON: {e}'}
                        self.wfile.write(json.dumps(error_response).encode('utf-8'))

                def log_message(self, format, *args):
                    # Suppress default logging
                    pass

            # Create server with collector reference
            server = HTTPServer(('localhost', self.port), lambda *args: WebhookHandler(*args))
            server.collector = self.signal_collector

            # Start server in background thread
            server_thread = threading.Thread(target=server.serve_forever, daemon=True)
            server_thread.start()

            self.server = server
            print(f"Webhook interceptor started on port {self.port}")

        except ImportError:
            print("Warning: http.server not available, webhook interception disabled")
        except Exception as e:
            print(f"Error starting webhook interceptor: {e}")


def main():
    """Main entry point for signal collection."""
    parser = argparse.ArgumentParser(
        description='Record real-time trading signals for Chimera evaluation'
    )
    parser.add_argument(
        '--output-dir',
        type=str,
        default='/opt/chimera/evaluation/signals/realtime',
        help='Directory to store recorded signals'
    )
    parser.add_argument(
        '--duration-days',
        type=int,
        default=5,
        help='Recording duration in days'
    )
    parser.add_argument(
        '--max-file-size-mb',
        type=int,
        default=100,
        help='Maximum file size before rotation (MB)'
    )
    parser.add_argument(
        '--intercept-port',
        type=int,
        default=8090,
        help='Port for webhook interception'
    )
    parser.add_argument(
        '--test-signal',
        type=str,
        default=None,
        help='Send test signal and exit'
    )

    args = parser.parse_args()

    # Create signal collector
    collector = SignalCollector(
        output_dir=Path(args.output_dir),
        recording_duration_days=args.duration_days,
        max_file_size_mb=args.max_file_size_mb
    )

    # Start webhook interceptor
    interceptor = WebhookInterceptor(collector, args.intercept_port)
    interceptor.start()

    # Handle test signal mode
    if args.test_signal:
        test_signal = {
            'wallet_address': 'TestWallet1111111111111111111111111111111111',
            'token_address': 'TestToken111111111111111111111111111111111',
            'action': 'buy',
            'amount_sol': 0.1,
            'strategy': 'shield',
            'timestamp': datetime.now().isoformat()
        }
        collector.record_signal(test_signal, source='test')
        collector.process_signals()
        collector.stop_recording()
        collector.save_summary()
        print(f"Test signal recorded. Statistics: {collector.get_statistics()}")
        sys.exit(0)

    # Start recording
    collector.start_recording()

    try:
        # Process signals in background thread
        process_thread = threading.Thread(target=collector.process_signals, daemon=True)
        process_thread.start()

        print(f"Recording signals for {args.duration_days} days...")
        print("Press Ctrl+C to stop recording early")

        # Keep main thread alive
        while True:
            time.sleep(60)

            # Print periodic statistics
            stats = collector.get_statistics()
            print(f"Recording progress: {stats['total_signals']} signals recorded")

    except KeyboardInterrupt:
        print("\nStopping signal recording...")
        collector.stop_recording()
        collector.save_summary()

        # Print final statistics
        stats = collector.get_statistics()
        print(f"Recording Statistics:")
        print(f"  Total signals: {stats['total_signals']}")
        print(f"  Files created: {stats['files_created']}")
        print(f"  By source: {stats['signals_by_source']}")
        print(f"  By strategy: {stats['signals_by_strategy']}")
        print(f"  Recording duration: {stats['recording_start_time']} to {stats['recording_end_time']}")

    sys.exit(0)


if __name__ == '__main__':
    main()