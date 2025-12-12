#!/usr/bin/env python3
"""
File watcher for automatic roster merging.

This script watches for roster_new.db file changes and automatically
triggers a merge when the file is created or updated.

Can be run as a background service or via cron.
"""

import os
import sys
import time
from pathlib import Path
from watchdog.observers import Observer
from watchdog.events import FileSystemEventHandler

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent))

from core.auto_merge import auto_merge_roster


class RosterMergeHandler(FileSystemEventHandler):
    """Handler for roster file changes."""
    
    def __init__(self, roster_path: Path, api_url: str, operator_container: str):
        self.roster_path = roster_path
        self.api_url = api_url
        self.operator_container = operator_container
        self.last_modified = 0
        self.debounce_seconds = 2.0  # Wait 2 seconds after file change
    
    def on_modified(self, event):
        """Handle file modification events."""
        if event.is_directory:
            return
        
        if Path(event.src_path) == self.roster_path:
            current_time = time.time()
            
            # Debounce: only process if file hasn't been modified recently
            if current_time - self.last_modified < self.debounce_seconds:
                return
            
            self.last_modified = current_time
            
            # Wait for debounce period to ensure file is fully written
            time.sleep(self.debounce_seconds)
            
            print(f"[Watcher] Detected roster file change: {event.src_path}")
            self.trigger_merge()
    
    def on_created(self, event):
        """Handle file creation events."""
        if event.is_directory:
            return
        
        if Path(event.src_path) == self.roster_path:
            print(f"[Watcher] Detected roster file creation: {event.src_path}")
            time.sleep(self.debounce_seconds)
            self.trigger_merge()
    
    def trigger_merge(self):
        """Trigger roster merge."""
        if not self.roster_path.exists():
            print(f"[Watcher] Roster file does not exist: {self.roster_path}")
            return
        
        print(f"[Watcher] Triggering automatic roster merge...")
        success, message = auto_merge_roster(
            roster_path=str(self.roster_path),
            api_url=self.api_url,
            operator_container=self.operator_container,
            prefer_api=True,
            retries=3,
        )
        
        if success:
            print(f"[Watcher] ✓ {message}")
        else:
            print(f"[Watcher] ✗ Merge failed: {message}")


def main():
    """Main entry point for file watcher."""
    import argparse
    
    parser = argparse.ArgumentParser(
        description="Watch for roster_new.db changes and auto-merge"
    )
    parser.add_argument(
        "--roster-path",
        type=str,
        default="../data/roster_new.db",
        help="Path to roster_new.db file to watch",
    )
    parser.add_argument(
        "--watch-dir",
        type=str,
        default="../data",
        help="Directory to watch (default: ../data)",
    )
    parser.add_argument(
        "--api-url",
        type=str,
        default=os.getenv("CHIMERA_API_URL", "http://localhost:8080"),
        help="Operator API URL",
    )
    parser.add_argument(
        "--operator-container",
        type=str,
        default=os.getenv("CHIMERA_OPERATOR_CONTAINER", "chimera-operator"),
        help="Docker container name for operator",
    )
    
    args = parser.parse_args()
    
    roster_path = Path(args.roster_path).resolve()
    watch_dir = Path(args.watch_dir).resolve()
    
    if not watch_dir.exists():
        print(f"Error: Watch directory does not exist: {watch_dir}")
        sys.exit(1)
    
    print(f"[Watcher] Starting roster file watcher...")
    print(f"  Watch directory: {watch_dir}")
    print(f"  Roster file: {roster_path}")
    print(f"  API URL: {args.api_url}")
    print(f"  Operator container: {args.operator_container}")
    print("")
    
    # Create event handler
    event_handler = RosterMergeHandler(
        roster_path=roster_path,
        api_url=args.api_url,
        operator_container=args.operator_container,
    )
    
    # Create observer
    observer = Observer()
    observer.schedule(event_handler, str(watch_dir), recursive=False)
    observer.start()
    
    try:
        print("[Watcher] Watching for roster file changes... (Press Ctrl+C to stop)")
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        print("\n[Watcher] Stopping watcher...")
        observer.stop()
    
    observer.join()
    print("[Watcher] Watcher stopped")


if __name__ == "__main__":
    main()
