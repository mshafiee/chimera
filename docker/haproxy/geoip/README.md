# GeoIP Database Setup

GeoIP databases are downloaded at image build time from the [P3TERX/GeoLite.mmdb](https://github.com/P3TERX/GeoLite.mmdb) community mirror and baked into the `geoip-lookup` service image.

## Database Download

- **Source**: GitHub mirror `P3TERX/GeoLite.mmdb` (updated daily via GitHub Actions)
- **Build-time**: Databases are downloaded during `docker compose build` using BuildKit cache mounts
- **Persistence**: BuildKit cache persists databases across rebuilds on the same host
- **No license key required**: Uses community mirror instead of MaxMind's licensed API

## Database Files

- **GeoLite2-Country.mmdb** (~8.4 MB): Country-level geolocation data
- **GeoLite2-ASN.mmdb** (~9 MB): Autonomous System Number data

Note: City-level database is not included (out of scope, ~59 MB). The `geoip-lookup` service falls back to Country database when City is unavailable.

## Refreshing Databases

### Force Re-download

To force a fresh download from the mirror:

```bash
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build --build-arg GEOIP_DB_REFRESH=1 geoip-lookup
```

### Normal Rebuild

A normal rebuild uses cached databases (fast):

```bash
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build geoip-lookup
```

## BuildKit Requirements

The `geoip-lookup` Dockerfile requires BuildKit (`# syntax=docker/dockerfile:1.6`). BuildKit is enabled by default in:
- Docker Desktop
- OrbStack
- Most modern Docker installations

If BuildKit is not enabled, set the environment variable:

```bash
export DOCKER_BUILDKIT=1
```

## Build Arguments

The `geoip-lookup` service supports optional build arguments in `docker-compose-haproxy.yml`:

```yaml
geoip-lookup:
  build:
    args:
      - GEOIP_DB_REFRESH=0                    # Force re-download (1=yes, 0=no)
      - GEOIP_COUNTRY_URL=https://...         # Custom Country DB URL
      - GEOIP_ASN_URL=https://...             # Custom ASN DB URL
```

## Cache Failure Handling

- **Download succeeds**: Validates file size (>1 MB), promotes to cache, copies to `/geoip`
- **Download fails**: Keeps cached copy from previous build (if available)
- **No cache available**: Build fails with explicit error message

## License & Attribution

The GeoLite2 databases are free for use with attribution per:
- MaxMind GeoLite2 EULA
- CC BY-SA 4.0 (for P3TERX/GeoLite.mmdb mirror contributions)

The mirror is maintained by the community and updated daily via GitHub Actions.

## Alternative Mirrors

If the primary mirror becomes unavailable, alternative mirrors include:
- Loyalsoldier/geoip (if available)
- Other GitHub-hosted GeoLite2 mirrors

## Troubleshooting

**Build fails with "no valid DB available":**
- Check network connectivity to GitHub
- Verify the mirror URL is still accessible
- Check BuildKit cache permissions

**Stale geolocation data:**
- Force re-download with `GEOIP_DB_REFRESH=1`
- Databases are updated daily in the mirror; monthly refresh recommended

**High memory usage:**
- GeoIP databases are memory-mapped for performance (~17 MB total)
- Redis cache adds additional overhead
- Reduce `CACHE_TTL` if memory constrained

**Verification:**
```bash
# Check databases are present in running container
docker run --rm chimera-geoip-lookup ls -la /geoip/

# Test lookup service
docker run --rm chimera-geoip-lookup python -c \
  "import geoip2.database as d; print(d.Reader('/geoip/GeoLite2-Country.mmdb').country('8.8.8.8').country.iso_code)"
# Output: US