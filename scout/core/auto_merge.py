"""
Automatic roster merge module for Scout.

This module provides automatic merging of roster_new.db into the main database
without manual intervention. It handles database locks gracefully with retries.
"""

import os
import sys
import time
import requests
import subprocess
from pathlib import Path
from typing import Optional, Tuple


def merge_via_api(
    api_url: str = "http://localhost:8080",
    roster_path: Optional[str] = None,
    timeout: int = 30,
    retries: int = 3,
    retry_delay: float = 2.0,
) -> Tuple[bool, str]:
    """
    Merge roster via Operator API endpoint.
    
    Args:
        api_url: Operator API base URL
        roster_path: Optional custom roster path
        timeout: Request timeout in seconds
        retries: Number of retry attempts
        retry_delay: Delay between retries in seconds
        
    Returns:
        Tuple of (success: bool, message: str)
    """
    endpoint = f"{api_url}/api/v1/roster/merge"
    
    payload = {}
    if roster_path:
        payload["roster_path"] = roster_path
    
    for attempt in range(retries):
        try:
            response = requests.post(
                endpoint,
                json=payload,
                timeout=timeout,
                headers={"Content-Type": "application/json"},
            )
            
            if response.status_code == 200:
                result = response.json()
                wallets_merged = result.get("wallets_merged", 0)
                return True, f"Successfully merged {wallets_merged} wallets via API"
            elif response.status_code == 401:
                # Authentication required - try SIGHUP instead
                return False, "API requires authentication, trying SIGHUP..."
            elif response.status_code == 500:
                # Server error - might be database lock, retry
                if attempt < retries - 1:
                    time.sleep(retry_delay * (attempt + 1))
                    continue
                return False, f"Server error: {response.text}"
            else:
                return False, f"API returned status {response.status_code}: {response.text}"
                
        except requests.exceptions.ConnectionError:
            if attempt < retries - 1:
                time.sleep(retry_delay * (attempt + 1))
                continue
            return False, "Could not connect to operator API"
        except requests.exceptions.Timeout:
            if attempt < retries - 1:
                time.sleep(retry_delay * (attempt + 1))
                continue
            return False, "API request timed out"
        except Exception as e:
            return False, f"API request failed: {str(e)}"
    
    return False, "All retry attempts failed"


def merge_via_sighup(
    operator_container: str = "chimera-operator",
    timeout: int = 10,
) -> Tuple[bool, str]:
    """
    Merge roster by sending SIGHUP to operator process.
    
    This works by finding the operator process and sending SIGHUP signal,
    which triggers the built-in roster merge handler.
    
    Args:
        operator_container: Docker container name for operator
        timeout: Timeout for finding process
        
    Returns:
        Tuple of (success: bool, message: str)
    """
    # Try to find operator process in container
    try:
        # Get PID from container
        result = subprocess.run(
            ["docker", "exec", operator_container, "pgrep", "-f", "chimera_operator"],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        
        if result.returncode == 0:
            pid = result.stdout.strip().split()[0]
            # Send SIGHUP to process in container
            subprocess.run(
                ["docker", "exec", operator_container, "kill", "-HUP", pid],
                check=True,
                timeout=timeout,
            )
            return True, f"Sent SIGHUP to operator process (PID: {pid})"
        else:
            # Fallback: try to send SIGHUP to container itself
            # Some setups might handle this
            try:
                subprocess.run(
                    ["docker", "kill", "-s", "HUP", operator_container],
                    check=True,
                    timeout=timeout,
                )
                return True, f"Sent SIGHUP to container {operator_container}"
            except subprocess.CalledProcessError:
                return False, "Could not find operator process or send SIGHUP"
                
    except subprocess.TimeoutExpired:
        return False, "Timeout finding operator process"
    except FileNotFoundError:
        return False, "Docker command not found"
    except Exception as e:
        return False, f"SIGHUP failed: {str(e)}"


def auto_merge_roster(
    roster_path: Optional[str] = None,
    api_url: str = "http://localhost:8080",
    operator_container: str = "chimera-operator",
    prefer_api: bool = True,
    retries: int = 3,
) -> Tuple[bool, str]:
    """
    Automatically merge roster using best available method.
    
    Tries API first (if prefer_api=True), then falls back to SIGHUP.
    Handles database locks with retries.
    
    Args:
        roster_path: Path to roster_new.db (defaults to ../data/roster_new.db)
        api_url: Operator API base URL
        operator_container: Docker container name for operator
        prefer_api: Whether to prefer API over SIGHUP
        retries: Number of retry attempts for API
        
    Returns:
        Tuple of (success: bool, message: str)
    """
    if roster_path is None:
        # Default path relative to scout directory
        scout_dir = Path(__file__).parent.parent
        roster_path = str(scout_dir.parent / "data" / "roster_new.db")
    
    roster_file = Path(roster_path)
    
    # Check if roster file exists
    if not roster_file.exists():
        return False, f"Roster file not found: {roster_path}"
    
    # Check if roster has wallets
    try:
        import sqlite3
        conn = sqlite3.connect(str(roster_file))
        cursor = conn.cursor()
        cursor.execute("SELECT COUNT(*) FROM wallets")
        count = cursor.fetchone()[0]
        conn.close()
        
        if count == 0:
            return False, "Roster file contains no wallets"
    except Exception as e:
        return False, f"Could not verify roster file: {str(e)}"
    
    # Try API first if preferred
    if prefer_api:
        success, message = merge_via_api(
            api_url=api_url,
            roster_path=roster_path,
            retries=retries,
        )
        if success:
            return True, message
        # If API failed due to auth, try SIGHUP
        if "authentication" in message.lower():
            print(f"[AutoMerge] API requires auth, trying SIGHUP...")
            return merge_via_sighup(operator_container=operator_container)
        # If API failed for other reasons, return the error
        return False, f"API merge failed: {message}"
    else:
        # Try SIGHUP first
        success, message = merge_via_sighup(operator_container=operator_container)
        if success:
            return True, message
        # Fallback to API
        print(f"[AutoMerge] SIGHUP failed, trying API...")
        return merge_via_api(
            api_url=api_url,
            roster_path=roster_path,
            retries=retries,
        )


if __name__ == "__main__":
    # CLI interface for testing
    import argparse
    
    parser = argparse.ArgumentParser(description="Auto-merge roster into main database")
    parser.add_argument(
        "--roster-path",
        type=str,
        help="Path to roster_new.db (default: ../data/roster_new.db)",
    )
    parser.add_argument(
        "--api-url",
        type=str,
        default="http://localhost:8080",
        help="Operator API URL (default: http://localhost:8080)",
    )
    parser.add_argument(
        "--operator-container",
        type=str,
        default="chimera-operator",
        help="Docker container name for operator (default: chimera-operator)",
    )
    parser.add_argument(
        "--prefer-sighup",
        action="store_true",
        help="Prefer SIGHUP over API",
    )
    parser.add_argument(
        "--retries",
        type=int,
        default=3,
        help="Number of retry attempts for API (default: 3)",
    )
    
    args = parser.parse_args()
    
    success, message = auto_merge_roster(
        roster_path=args.roster_path,
        api_url=args.api_url,
        operator_container=args.operator_container,
        prefer_api=not args.prefer_sighup,
        retries=args.retries,
    )
    
    if success:
        print(f"✓ {message}")
        sys.exit(0)
    else:
        print(f"✗ {message}")
        sys.exit(1)
