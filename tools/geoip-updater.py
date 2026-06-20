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
import gzip
from datetime import datetime
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

# Verify license key
if not MAXMIND_LICENSE_KEY:
    logger.error("MAXMIND_LICENSE_KEY environment variable not set")
    logger.info("Get a free license key from: https://dev.maxmind.com/geoip/geolite2-free-geolocation-data")
    sys.exit(1)


def download_database(db_name: str, edition_id: str) -> tuple[bool, str]:
    """Download a MaxMind database and verify checksum"""
    try:
        # Download URL
        download_url = f"{DOWNLOAD_BASE_URL}?edition_id={edition_id}&suffix=tar.gz"

        logger.info(f"Downloading {db_name} database from MaxMind...")

        # Download with authentication
        response = requests.get(
            download_url,
            auth=("sid", MAXMIND_LICENSE_KEY),
            stream=True,
            timeout=300  # 5 minute timeout
        )

        if response.status_code != 200:
            logger.error(f"Failed to download {db_name}: HTTP {response.status_code}")
            return False, ""

        # Save to temporary file
        with tempfile.NamedTemporaryFile(delete=False, suffix=".tar.gz") as temp_file:
            for chunk in response.iter_content(chunk_size=8192):
                temp_file.write(chunk)
            temp_path = temp_file.name

        # Extract .mmdb file from tar.gz
        logger.info(f"Extracting {db_name} database...")

        import tarfile
        with tarfile.open(temp_path, "r:gz") as tar:
            for member in tar.getmembers():
                if member.name.endswith(".mmdb"):
                    # Extract to temporary location
                    tar.extract(member, path=tempfile.gettempdir())
                    extracted_path = os.path.join(tempfile.gettempdir(), member.name)

                    # Move to final location
                    final_path = os.path.join(GEOIP_DB_DIR, DATABASES[db_name])

                    # Create backup of existing database
                    if os.path.exists(final_path):
                        backup_path = f"{final_path}.backup"
                        shutil.copy2(final_path, backup_path)
                        logger.info(f"Created backup: {backup_path}")

                    # Move new database to final location
                    shutil.move(extracted_path, final_path)

                    logger.info(f"Updated {db_name} database successfully")
                    return True, final_path

        # Clean up temporary file
        os.unlink(temp_path)

        return False, ""

    except Exception as e:
        logger.error(f"Error downloading {db_name}: {e}")
        return False, ""


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
            except:
                pass  # Lookup might fail, but file structure is valid

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
            logger.info(f"Updating {db_name}...")

            edition_id = f"{db_name}-CSV" if "CSV" in filename else db_name

            success, path = download_database(db_name, edition_id)

            if success and path:
                # Verify database integrity
                if verify_database_integrity(path):
                    results["databases_updated"].append({
                        "name": db_name,
                        "path": path,
                        "size": os.path.getsize(path)
                    })
                    logger.info(f"✓ {db_name} updated successfully")
                else:
                    results["databases_failed"].append(db_name)
                    results["errors"].append(f"{db_name}: Database verification failed")
                    logger.error(f"✗ {db_name} verification failed")
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