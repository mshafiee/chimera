#!/usr/bin/env python3
"""
Chimera GeoIP Database Updater
Automated updates for MaxMind GeoLite2 databases

This script:
1. Downloads the latest GeoLite2 City, Country, and ASN databases
2. Verifies checksums for security
3. Updates databases atomically
4. Can be run via cron for automated updates
"""

import os
import sys
import logging
import requests
import hashlib
import tempfile
import shutil
from datetime import datetime, timezone
from pathlib import Path

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Configuration
MAXMIND_LICENSE_KEY = os.getenv("MAXMIND_LICENSE_KEY", "")
GEOIP_DB_DIR = os.getenv("GEOIP_DB_DIR", "/geoip")
DOWNLOAD_BASE_URL = "https://download.maxmind.com/app/geoip_download"
DATABASES = {
    "GeoLite2-City": "GeoLite2-City.mmdb",
    "GeoLite2-Country": "GeoLite2-Country.mmdb",
    "GeoLite2-ASN": "GeoLite2-ASN.mmdb"
}
MAX_DB_AGE_DAYS = 7

# Verify license key
if not MAXMIND_LICENSE_KEY:
    logger.error("MAXMIND_LICENSE_KEY environment variable not set")
    logger.info("Get a free license key from: https://dev.maxmind.com/geoip/geolite2-free-geolocation-data")
    sys.exit(1)


def _should_update(final_path: str) -> bool:
    """Return True when the live database is missing or older than MAX_DB_AGE_DAYS."""
    if not os.path.exists(final_path):
        return True
    age_days = (datetime.now(timezone.utc).timestamp() - os.path.getmtime(final_path)) / 86400
    return age_days >= MAX_DB_AGE_DAYS


def download_database(db_name: str, edition_id: str) -> tuple[bool, str]:
    """Download a MaxMind database and verify its checksum before replacing
    the live copy. The final swap is atomic (os.replace on the same filesystem).
    """
    temp_path = None
    try:
        # MaxMind authenticates via the license_key query parameter
        download_url = f"{DOWNLOAD_BASE_URL}?edition_id={edition_id}&suffix=tar.gz&license_key={MAXMIND_LICENSE_KEY}"

        logger.info(f"Downloading {db_name} database from MaxMind...")

        response = requests.get(download_url, stream=True, timeout=300)
        if response.status_code != 200:
            logger.error(f"Failed to download {db_name}: HTTP {response.status_code}")
            return False, ""

        # Fetch the .sha256 sidecar and verify the archive digest
        sha_url = f"{DOWNLOAD_BASE_URL}?edition_id={edition_id}&suffix=tar.gz.sha256&license_key={MAXMIND_LICENSE_KEY}"
        sha_response = requests.get(sha_url, timeout=60)
        if sha_response.status_code != 200:
            logger.error(f"Failed to fetch checksum sidecar for {db_name}: HTTP {sha_response.status_code}")
            return False, ""
        expected_sha = sha_response.text.strip().split()[0].lower()
        if not expected_sha:
            logger.error(f"Empty checksum sidecar for {db_name}")
            return False, ""

        # Save to temporary file
        with tempfile.NamedTemporaryFile(delete=False, suffix=".tar.gz") as temp_file:
            for chunk in response.iter_content(chunk_size=8192):
                temp_file.write(chunk)
            temp_path = temp_file.name

        # Verify the downloaded archive checksum
        actual_sha = hashlib.sha256(open(temp_path, 'rb').read()).hexdigest()
        if actual_sha != expected_sha:
            logger.error(f"Checksum mismatch for {db_name}: expected {expected_sha}, got {actual_sha}")
            return False, ""

        # Extract to a temporary name inside GEOIP_DB_DIR (same filesystem so
        # the final os.replace is atomic)
        os.makedirs(GEOIP_DB_DIR, exist_ok=True)
        final_path = os.path.join(GEOIP_DB_DIR, DATABASES[db_name])
        tmp_final = final_path + ".tmp"
        if os.path.exists(tmp_final):
            os.unlink(tmp_final)

        import tarfile
        extracted_dir = None
        with tarfile.open(temp_path, "r:gz") as tar:
            for member in tar.getmembers():
                if member.name.endswith(".mmdb"):
                    # Defense-in-depth: reject unsafe archive member names
                    member_path = os.path.normpath(member.name)
                    if member_path.startswith("..") or os.path.isabs(member_path):
                        logger.error(f"Unsafe archive member name: {member.name}")
                        return False, ""
                    extracted_dir = os.path.join(GEOIP_DB_DIR, member_path.split(os.sep)[0])
                    tar.extract(member, path=GEOIP_DB_DIR)
                    os.replace(os.path.join(GEOIP_DB_DIR, member_path), tmp_final)
                    break

        if not os.path.exists(tmp_final):
            logger.error(f"No .mmdb file found in archive for {db_name}")
            return False, ""

        # Verify integrity BEFORE replacing the live file
        if not verify_database_integrity(tmp_final):
            logger.error(f"{db_name} verification failed; keeping the existing database")
            return False, ""

        # Create backup of the live database, then swap atomically
        if os.path.exists(final_path):
            backup_path = f"{final_path}.backup"
            shutil.copy2(final_path, backup_path)
            logger.info(f"Created backup: {backup_path}")

        os.replace(tmp_final, final_path)

        logger.info(f"Updated {db_name} database successfully")
        return True, final_path

    except Exception as e:
        logger.error(f"Error downloading {db_name}: {e}")
        return False, ""
    finally:
        # Always remove the downloaded archive
        if temp_path and os.path.exists(temp_path):
            os.unlink(temp_path)
        # Remove any leftover extraction directory
        if extracted_dir and os.path.isdir(extracted_dir) and os.path.basename(extracted_dir) != os.path.basename(final_path):
            shutil.rmtree(extracted_dir, ignore_errors=True)


def verify_database_integrity(db_path: str) -> bool:
    """Verify the database file is valid"""
    try:
        if not os.path.exists(db_path):
            logger.error(f"Database file not found: {db_path}")
            return False

        # Check file size (should be > 1MB)
        file_size = os.path.getsize(db_path)
        if file_size < 1024 * 1024:  # Less than 1MB
            logger.error(f"Database file too small: {file_size} bytes")
            return False

        # Try to open with geoip2 to verify it's a valid MMDB file
        try:
            import geoip2.database
            reader = geoip2.database.Reader(db_path)

            # Try a test lookup based on database type
            try:
                if "City" in db_path:
                    reader.city("8.8.8.8")
                elif "Country" in db_path:
                    reader.country("8.8.8.8")
                elif "ASN" in db_path:
                    reader.asn("8.8.8.8")
            except geoip2.errors.AddressNotFoundError:
                pass  # No record for 8.8.8.8; file structure is valid
            except Exception as lookup_err:
                logger.error(f"Test lookup failed: {lookup_err}")

            reader.close()

            logger.info(f"Database integrity verified: {db_path}")
            return True

        except Exception as e:
            logger.error(f"Database verification failed: {e}")
            return False

    except Exception as e:
        logger.error(f"Error verifying database: {e}")
        return False


def update_databases(force: bool = False) -> dict:
    """Update all GeoIP databases"""
    results = {
        "timestamp": datetime.now().isoformat(),
        "databases_updated": [],
        "databases_failed": [],
        "errors": []
    }

    # Create database directory if it doesn't exist
    os.makedirs(GEOIP_DB_DIR, exist_ok=True)

    logger.info("Starting GeoIP database update...")

    for db_name, filename in DATABASES.items():
        try:
            final_path = os.path.join(GEOIP_DB_DIR, filename)

            # Respect the recency check unless --force was given
            if not force and not _should_update(final_path):
                logger.info(f"Skipping {db_name}: database is recent (use --force to override)")
                continue

            logger.info(f"Updating {db_name}...")

            edition_id = f"{db_name}-CSV" if "CSV" in filename else db_name

            success, path = download_database(db_name, edition_id)

            if success and path:
                results["databases_updated"].append({
                    "name": db_name,
                    "path": path,
                    "size": os.path.getsize(path)
                })
                logger.info(f"✓ {db_name} updated successfully")
            else:
                results["databases_failed"].append(db_name)
                results["errors"].append(f"{db_name}: Download failed")
                logger.error(f"✗ {db_name} download failed")

        except Exception as e:
            results["databases_failed"].append(db_name)
            results["errors"].append(f"{db_name}: {str(e)}")
            logger.error(f"✗ {db_name} update failed: {e}")

    logger.info(f"Update complete: {len(results['databases_updated'])} updated, {len(results['databases_failed'])} failed")

    return results


def main():
    """Main entry point"""
    import argparse

    parser = argparse.ArgumentParser(description="Update MaxMind GeoIP databases")
    parser.add_argument("--force", action="store_true", help="Force update even if recent")
    parser.add_argument("--verify", action="store_true", help="Only verify existing databases")
    parser.add_argument("--list", action="store_true", help="List current database files")

    args = parser.parse_args()

    if args.list:
        print("Current GeoIP databases:")
        for db_name, filename in DATABASES.items():
            db_path = os.path.join(GEOIP_DB_DIR, filename)
            if os.path.exists(db_path):
                size = os.path.getsize(db_path)
                mtime = datetime.fromtimestamp(os.path.getmtime(db_path))
                print(f"  {db_name}: {size:,} bytes, modified {mtime}")
            else:
                print(f"  {db_name}: Not found")
        return

    if args.verify:
        print("Verifying existing databases...")
        all_valid = True
        for db_name, filename in DATABASES.items():
            db_path = os.path.join(GEOIP_DB_DIR, filename)
            if os.path.exists(db_path):
                valid = verify_database_integrity(db_path)
                status = "✓ Valid" if valid else "✗ Invalid"
                print(f"  {db_name}: {status}")
                if not valid:
                    all_valid = False
            else:
                print(f"  {db_name}: Not found")
                all_valid = False

        if all_valid:
            print("\n✓ All databases are valid")
        else:
            print("\n✗ Some databases are invalid or missing")
            sys.exit(1)
        return

    # Perform update
    results = update_databases(force=args.force)

    # Print summary
    print(f"\nUpdate Summary:")
    print(f"  Updated: {len(results['databases_updated'])} databases")
    print(f"  Failed: {len(results['databases_failed'])} databases")

    if results['databases_updated']:
        print(f"\nSuccessfully updated:")
        for db in results['databases_updated']:
            print(f"  ✓ {db['name']}: {db['size']:,} bytes")

    if results['databases_failed']:
        print(f"\nFailed to update:")
        for db in results['databases_failed']:
            print(f"  ✗ {db}")

    # Exit with error code if any failures
    sys.exit(0 if not results['databases_failed'] else 1)


if __name__ == "__main__":
    main()
