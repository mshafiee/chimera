#!/usr/bin/env python3
"""
Chimera GeoIP Lookup Service
Provides IP geolocation using MaxMind GeoLite2 database with Redis caching

This service:
1. Looks up IP addresses to get city, country, ASN, and organization
2. Caches results in Redis for 1 hour
3. Exposes metrics for Prometheus scraping
4. Supports automated database updates
5. Provides policy evaluation for access control
"""

from fastapi import FastAPI, HTTPException
from prometheus_client import Counter, Histogram, Gauge, generate_latest, CONTENT_TYPE_LATEST
from prometheus_fastapi.instrumentator import Instrumentator
from pydantic import BaseModel
from typing import Optional, Dict, Any
import redis
import uvicorn
import logging
import os
import geoip2.database
import geoip2.errors
import json
from datetime import datetime
import asyncio

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Configuration
REDIS_HOST = os.getenv("REDIS_HOST", "localhost")
REDIS_PORT = int(os.getenv("REDIS_PORT", "6379"))
METRICS_PORT = int(os.getenv("METRICS_PORT", "8001"))
GEOIP_CITY_DB_PATH = os.getenv("GEOIP_CITY_DB_PATH", "/geoip/GeoLite2-City.mmdb")
GEOIP_COUNTRY_DB_PATH = os.getenv("GEOIP_COUNTRY_DB_PATH", "/geoip/GeoLite2-Country.mmdb")
GEOIP_ASN_DB_PATH = os.getenv("GEOIP_ASN_DB_PATH", "/geoip/GeoLite2-ASN.mmdb")
CACHE_TTL = int(os.getenv("CACHE_TTL", "3600"))  # 1 hour default
POLICY_CACHE_TTL = int(os.getenv("POLICY_CACHE_TTL", "300"))  # 5 minutes for policy decisions

# FastAPI app
app = FastAPI(
    title="Chimera GeoIP Lookup Service",
    description="IP geolocation service with MaxMind GeoLite2 and Redis caching",
    version="1.0.0"
)

# Instrumentator for Prometheus metrics
instrumentator = Instrumentator(app)

# Redis connection for caching
try:
    redis_client = redis.Redis(
        host=REDIS_HOST,
        port=REDIS_PORT,
        decode_responses=True,
        socket_connect_timeout=5,
        health_check_interval=30
    )
    redis_client.ping()
    logger.info(f"Connected to Redis at {REDIS_HOST}:{REDIS_PORT}")
except Exception as e:
    logger.warning(f"Redis connection failed: {e}. Running without cache.")
    redis_client = None

# GeoIP database readers
city_reader = None
country_reader = None
asn_reader = None

try:
    # Try to load City database first (most detailed)
    if os.path.exists(GEOIP_CITY_DB_PATH):
        city_reader = geoip2.database.Reader(GEOIP_CITY_DB_PATH)
        logger.info(f"Loaded GeoIP City database from {GEOIP_CITY_DB_PATH}")
    else:
        logger.warning(f"GeoIP City database not found at {GEOIP_CITY_DB_PATH}")

    # Fallback to Country database if City not available
    if not city_reader and os.path.exists(GEOIP_COUNTRY_DB_PATH):
        country_reader = geoip2.database.Reader(GEOIP_COUNTRY_DB_PATH)
        logger.info(f"Loaded GeoIP Country database from {GEOIP_COUNTRY_DB_PATH}")
    elif not city_reader:
        logger.warning(f"GeoIP Country database not found at {GEOIP_COUNTRY_DB_PATH}")

    if os.path.exists(GEOIP_ASN_DB_PATH):
        asn_reader = geoip2.database.Reader(GEOIP_ASN_DB_PATH)
        logger.info(f"Loaded GeoIP ASN database from {GEOIP_ASN_DB_PATH}")
    else:
        logger.warning(f"GeoIP ASN database not found at {GEOIP_ASN_DB_PATH}")

except Exception as e:
    logger.error(f"Failed to load GeoIP databases: {e}")

# Prometheus Metrics
geoip_lookups_total = Counter(
    "chimera_geoip_lookups_total",
    ["cache_status", "db_type"],
    "Total GeoIP lookup requests"
)

geoip_lookup_duration = Histogram(
    "chimera_geoip_lookup_duration_seconds",
    "Time taken to perform GeoIP lookup"
)

geoip_cache_hits = Gauge(
    "chimera_geoip_cache_hits",
    "Number of cache hits vs misses"
)

geoip_database_age = Gauge(
    "chimera_geoip_database_age_hours",
    ["database_type"],
    "Age of GeoIP database in hours"
)

geoip_policy_evaluations_total = Counter(
    "chimera_geoip_policy_evaluations_total",
    ["decision", "policy_type"],
    "Total policy evaluations for access control"
)

# Pydantic models
class GeoIPInfo(BaseModel):
    ip_address: str
    city: Optional[str] = None
    subdivision: Optional[str] = None  # State/Province
    country_code: Optional[str] = None
    country_name: Optional[str] = None
    continent_code: Optional[str] = None
    latitude: Optional[float] = None
    longitude: Optional[float] = None
    timezone: Optional[str] = None
    asn: Optional[str] = None
    asn_organization: Optional[str] = None
    cache_status: str
    lookup_time: float

class PolicyDecision(BaseModel):
    ip_address: str
    decision: str  # "allow" or "deny"
    reason: str
    policy_type: str
    details: Dict[str, Any]

class PolicyResponse(BaseModel):
    status: str
    decision: Optional[PolicyDecision] = None
    error: Optional[str] = None

class GeoIPResponse(BaseModel):
    status: str
    data: Optional[GeoIPInfo] = None
    error: Optional[str] = None

async def lookup_city(ip_address: str) -> Dict[str, Any]:
    """Lookup city information for IP address (most detailed)"""
    if not city_reader:
        return {}

    try:
        response = city_reader.city(ip_address)
        result = {
            "city": response.city.name if response.city.name else None,
            "subdivision": response.subdivisions.most_specific.name if response.subdivisions.most_specific else None,
            "country_code": response.country.iso_code,
            "country_name": response.country.name,
            "continent_code": response.continent.code,
            "latitude": response.location.latitude if response.location else None,
            "longitude": response.location.longitude if response.location else None,
            "timezone": response.location.time_zone if response.location else None
        }
        return result
    except geoip2.errors.AddressNotFoundError:
        logger.debug(f"IP address not found in city database: {ip_address}")
        return {}
    except Exception as e:
        logger.error(f"Error looking up city: {e}")
        return {}

async def lookup_country(ip_address: str) -> Dict[str, Any]:
    """Lookup country information for IP address (fallback)"""
    if not country_reader:
        return {
            "country_code": "unknown",
            "country_name": "Unknown",
            "continent_code": "unknown"
        }

    try:
        response = country_reader.country(ip_address)
        return {
            "country_code": response.country.iso_code,
            "country_name": response.country.name,
            "continent_code": response.continent.code
        }
    except geoip2.errors.AddressNotFoundError:
        logger.debug(f"IP address not found in database: {ip_address}")
        return {
            "country_code": "unknown",
            "country_name": "Unknown",
            "continent_code": "unknown"
        }
    except Exception as e:
        logger.error(f"Error looking up country: {e}")
        return {
            "country_code": "error",
            "country_name": "Error",
            "continent_code": "error"
        }

async def lookup_asn(ip_address: str) -> Dict[str, Any]:
    """Lookup ASN information for IP address"""
    if not asn_reader:
        return {
            "asn": "unknown",
            "asn_organization": "Unknown"
        }

    try:
        response = asn_reader.asn(ip_address)
        return {
            "asn": f"AS{response.autonomous_system_number}",
            "asn_organization": response.autonomous_system_organization
        }
    except geoip2.errors.AddressNotFoundError:
        logger.debug(f"IP address not found in ASN database: {ip_address}")
        return {
            "asn": "unknown",
            "asn_organization": "Unknown"
        }
    except Exception as e:
        logger.error(f"Error looking up ASN: {e}")
        return {
            "asn": "error",
            "asn_organization": "Error"
        }

async def get_cached_geoip(ip_address: str) -> Optional[Dict[str, Any]]:
    """Get GeoIP data from cache"""
    if not redis_client:
        return None

    try:
        cache_key = f"geoip:{ip_address}"
        cached_data = redis_client.get(cache_key)
        if cached_data:
            geoip_lookups_total.labels(cache_status="hit", db_type="combined").inc()
            return json.loads(cached_data)
    except Exception as e:
        logger.error(f"Error getting cached data: {e}")

    return None

async def set_cached_geoip(ip_address: str, data: Dict[str, Any]):
    """Set GeoIP data in cache"""
    if not redis_client:
        return

    try:
        cache_key = f"geoip:{ip_address}"
        redis_client.setex(cache_key, CACHE_TTL, json.dumps(data))
    except Exception as e:
        logger.error(f"Error setting cached data: {e}")

async def perform_geoip_lookup(ip_address: str) -> Dict[str, Any]:
    """Perform complete GeoIP lookup with caching"""
    start_time = datetime.now()

    # Check cache first
    cached_data = await get_cached_geoip(ip_address)
    if cached_data:
        return cached_data

    # Perform lookup
    geoip_lookups_total.labels(cache_status="miss", db_type="combined").inc()

    # Try city lookup first (most detailed)
    city_data = await lookup_city(ip_address)

    # If city lookup succeeded, use that data
    if city_data:
        asn_data = await lookup_asn(ip_address)

        result = {
            "ip_address": ip_address,
            "city": city_data.get("city"),
            "subdivision": city_data.get("subdivision"),
            "country_code": city_data.get("country_code"),
            "country_name": city_data.get("country_name"),
            "continent_code": city_data.get("continent_code"),
            "latitude": city_data.get("latitude"),
            "longitude": city_data.get("longitude"),
            "timezone": city_data.get("timezone"),
            "asn": asn_data.get("asn"),
            "asn_organization": asn_data.get("asn_organization"),
            "cache_status": "miss",
            "lookup_time": (datetime.now() - start_time).total_seconds()
        }
    else:
        # Fallback to country-level lookup
        country_data = await lookup_country(ip_address)
        asn_data = await lookup_asn(ip_address)

        result = {
            "ip_address": ip_address,
            "city": None,
            "subdivision": None,
            "country_code": country_data.get("country_code"),
            "country_name": country_data.get("country_name"),
            "continent_code": country_data.get("continent_code"),
            "latitude": None,
            "longitude": None,
            "timezone": None,
            "asn": asn_data.get("asn"),
            "asn_organization": asn_data.get("asn_organization"),
            "cache_status": "miss",
            "lookup_time": (datetime.now() - start_time).total_seconds()
        }

    # Cache the result
    await set_cached_geoip(ip_address, result)

    return result

# API Endpoints
@app.get("/health")
async def health_check():
    """Health check endpoint"""
    health_status = {
        "status": "healthy",
        "redis_connected": redis_client is not None,
        "city_db_loaded": city_reader is not None,
        "country_db_loaded": country_reader is not None,
        "asn_db_loaded": asn_reader is not None,
        "geoip_available": city_reader is not None or country_reader is not None
    }

    if not health_status["geoip_available"]:
        raise HTTPException(status_code=503, detail=health_status)

    return health_status

@app.get("/metrics")
async def metrics():
    """Prometheus metrics endpoint"""
    return Response(content=generate_latest(), media_type=CONTENT_TYPE_LATEST)

@app.get("/geoip/{ip_address}", response_model=GeoIPResponse)
async def get_geoip(ip_address: str):
    """Get GeoIP information for an IP address"""
    try:
        # Basic IP validation
        if not ip_address or len(ip_address) < 7:
            raise HTTPException(status_code=400, detail="Invalid IP address")

        # Perform lookup
        result = await perform_geoip_lookup(ip_address)

        return GeoIPResponse(
            status="success",
            data=GeoIPInfo(**result)
        )

    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"Error processing GeoIP request: {e}")
        return GeoIPResponse(
            status="error",
            error=str(e)
        )

@app.get("/geoip/batch", response_model=Dict[str, Any])
async def get_batch_geoip(ip_addresses: str):
    """Get GeoIP information for multiple IP addresses (comma-separated)"""
    try:
        ips = [ip.strip() for ip in ip_addresses.split(",")]
        results = []

        for ip in ips:
            result = await perform_geoip_lookup(ip)
            results.append(result)

        return {
            "status": "success",
            "count": len(results),
            "data": results
        }

    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error processing batch request: {e}")

@app.get("/cache/stats")
async def get_cache_stats():
    """Get cache statistics"""
    if not redis_client:
        return {"error": "Redis not available"}

    try:
        info = redis_client.info("stats")
        return {
            "cache_hits": info.get("keyspace_hits", 0),
            "cache_misses": info.get("keyspace_misses", 0),
            "total_keys": redis_client.dbsize()
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error getting cache stats: {e}")

@app.post("/cache/clear")
async def clear_cache():
    """Clear the GeoIP cache"""
    if not redis_client:
        return {"error": "Redis not available"}

    try:
        # Delete all geoip keys
        keys = redis_client.keys("geoip:*")
        if keys:
            redis_client.delete(*keys)

        return {
            "status": "success",
            "keys_cleared": len(keys)
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error clearing cache: {e}")

@app.get("/geoip/evaluate/{ip_address}", response_model=PolicyResponse)
async def evaluate_policy(ip_address: str, policy_type: str = "default"):
    """
    Evaluate access policy for an IP address

    Policy types:
    - default: Basic geographic evaluation
    - strict: Strict country restrictions
    - permissive: Allow most countries
    """
    try:
        # Basic IP validation
        if not ip_address or len(ip_address) < 7:
            raise HTTPException(status_code=400, detail="Invalid IP address")

        # Perform GeoIP lookup
        geoip_data = await perform_geoip_lookup(ip_address)

        # Define default blocked countries
        blocked_countries = {"CN", "RU", "KP", "IR"}
        allowed_countries = {"US", "GB", "DE", "FR", "JP", "SG", "CH", "CA", "AU"}

        decision = "allow"
        reason = "IP address meets access policy requirements"
        details = {
            "country_code": geoip_data.get("country_code"),
            "city": geoip_data.get("city"),
            "policy_type": policy_type
        }

        # Evaluate based on policy type
        if policy_type == "strict":
            # Strict mode - only allow whitelisted countries
            if geoip_data.get("country_code") not in allowed_countries:
                decision = "deny"
                reason = f"Country {geoip_data.get('country_code')} not in strict whitelist"

        elif policy_type == "default":
            # Default mode - block blacklisted countries
            if geoip_data.get("country_code") in blocked_countries:
                decision = "deny"
                reason = f"Country {geoip_data.get('country_code')} is blocked by default policy"

        # Update metrics
        geoip_policy_evaluations_total.labels(
            decision=decision,
            policy_type=policy_type
        ).inc()

        return PolicyResponse(
            status="success",
            decision=PolicyDecision(
                ip_address=ip_address,
                decision=decision,
                reason=reason,
                policy_type=policy_type,
                details=details
            )
        )

    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"Error evaluating policy: {e}")
        return PolicyResponse(
            status="error",
            error=str(e)
        )

# Startup event
@app.on_event("startup")
async def startup_event():
    """Initialize GeoIP service on startup"""
    logger.info("Starting Chimera GeoIP Lookup Service")

    # Log available databases
    if city_reader:
        logger.info("✓ City-level GeoIP lookups available")
    elif country_reader:
        logger.info("✓ Country-level GeoIP lookups available (city not available)")
    else:
        logger.warning("✗ No GeoIP databases available")

    if asn_reader:
        logger.info("✓ ASN lookups available")

    # Check database age
    try:
        if city_reader:
            geoip_database_age.labels(database_type="city").set(0)
        if country_reader:
            geoip_database_age.labels(database_type="country").set(0)
        if asn_reader:
            geoip_database_age.labels(database_type="asn").set(0)
    except Exception as e:
        logger.warning(f"Could not determine database age: {e}")

# Shutdown event
@app.on_event("shutdown")
async def shutdown_event():
    """Clean up on shutdown"""
    global city_reader, country_reader, asn_reader

    if city_reader:
        city_reader.close()
        logger.info("Closed GeoIP city database")

    if country_reader:
        country_reader.close()
        logger.info("Closed GeoIP country database")

    if asn_reader:
        asn_reader.close()
        logger.info("Closed GeoIP ASN database")

    logger.info("GeoIP lookup service shutting down")

if __name__ == "__main__":
    uvicorn.run(
        "geoip_lookup:app",
        host="0.0.0.0",
        port=METRICS_PORT,
        log_level="info"
    )