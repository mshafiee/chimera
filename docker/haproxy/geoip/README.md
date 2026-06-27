# GeoIP Database Setup

This directory contains MaxMind GeoLite2 databases for IP geolocation.

## Setup Instructions

### 1. Get MaxMind License Key

1. Visit [MaxMind Developer Portal](https://dev.maxmind.com/geoip/geolite2-free-geolocation-data)
2. Create a free account and generate a license key
3. Set the `MAXMIND_LICENSE_KEY` environment variable

### 2. Initialize Databases

Run the GeoIP updater manually to download initial databases:

```bash
docker run --rm \
  -e MAXMIND_LICENSE_KEY=your_license_key_here \
  -v $(pwd)/docker/haproxy/geoip:/geoip \
  chimera-geoip-updater:latest \
  python /app/geoip-updater.py
```

Or using the tools directly:

```bash
MAXMIND_LICENSE_KEY=your_key python tools/geoip-updater.py
```

### 3. Verify Databases

Check that the database files are present:

```bash
ls -la docker/haproxy/geoip/
```

Expected files:
- `GeoLite2-Country.mmdb` (~5MB)
- `GeoLite2-ASN.mmdb` (~10MB)

### 4. Configure HAProxy

Update the HAProxy configuration to use the GeoIP service for geolocation enrichment.

The GeoIP lookup service will be available at `http://geoip-lookup:8001/geoip/{ip_address}`

### 5. Automatic Updates

The GeoIP updater service runs weekly (Sundays at 3 AM) to keep databases current.

## Environment Variables

- `MAXMIND_LICENSE_KEY`: Your MaxMind license key (required)
- `GEOIP_DB_DIR`: Path to store database files (default: `/geoip`)
- `UPDATE_SCHEDULE`: Cron schedule for updates (default: weekly)

## Database Files

- **GeoLite2-Country.mmdb**: Country-level geolocation data
- **GeoLite2-ASN.mmdb**: Autonomous System Number data

## License

The GeoLite2 databases are free for use with attribution. See MaxMind's license terms for details.

## Troubleshooting

**Databases not loading:**
- Check that the MAXMIND_LICENSE_KEY is set correctly
- Verify network connectivity to MaxMind servers
- Check file permissions in the geoip directory

**Stale geolocation data:**
- Run the updater manually: `python tools/geoip-updater.py --force`
- Check the updater logs for any errors

**High memory usage:**
- GeoIP databases are memory-mapped for performance
- Normal usage is ~15-20MB per database file
- Reduce Redis cache size if memory constrained