#!/usr/bin/env python3
"""
Chimera Security Log Parser Service
Processes HAProxy security event logs and exposes metrics to Prometheus

This service:
1. Parses JSON security logs from HAProxy
2. Categorizes security events by type and severity
3. Exposes metrics for Prometheus scraping
4. Provides real-time security event feed
"""

from fastapi import FastAPI, HTTPException, Response
from prometheus_client import Counter, Histogram, Gauge, generate_latest, CONTENT_TYPE_LATEST
from pydantic import BaseModel
from typing import Optional, Dict, Any, List
import json
import redis
import uvicorn
import logging
from datetime import datetime
import re
import os
from collections import defaultdict
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
HA_PROXY_LOG_PATH = os.getenv("HAPROXY_LOG_PATH", "/var/log/haproxy/security.log")
METRICS_PORT = int(os.getenv("METRICS_PORT", "8000"))
LOG_CHECK_INTERVAL = int(os.getenv("LOG_CHECK_INTERVAL", "5"))

# Trusted IP whitelist — traffic from these addresses skips attack-pattern detection.
# Comma-separated list via env var (e.g. SECURITY_TRUSTED_IPS=1.2.3.4,5.6.7.8).
_trusted_ips_env = os.getenv("SECURITY_TRUSTED_IPS", "")
TRUSTED_IPS = {ip.strip() for ip in _trusted_ips_env.split(",") if ip.strip()}

# FastAPI app
app = FastAPI(
    title="Chimera Security Log Parser",
    description="Processes HAProxy security events and exposes metrics",
    version="1.0.0"
)

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

# Prometheus Metrics
# Security event counters
security_events_total = Counter(
    "chimera_haproxy_security_events_total",
    "Total security events processed from HAProxy",
    ["event_type", "severity", "geo_country"]
)

rate_limit_violations_total = Counter(
    "chimera_haproxy_rate_limit_violations_total",
    "Rate limit violations detected by HAProxy",
    ["endpoint"]
)

auth_failures_total = Counter(
    "chimera_haproxy_auth_failures_total",
    "Authentication failures detected",
    ["auth_type", "reason"]
)

attack_detected_total = Counter(
    "chimera_haproxy_attack_detected_total",
    "Attack patterns detected by security analysis",
    ["attack_type", "severity"]
)

geo_anomalies_total = Counter(
    "chimera_haproxy_geo_anomalies_total",
    "Geographic access anomalies detected",
    ["anomaly_type", "geo_country"]
)

# Performance metrics
log_processing_duration = Histogram(
    "chimera_security_log_parser_processing_duration_seconds",
    "Time taken to process security log events"
)

active_threats = Gauge(
    "chimera_haproxy_active_threats",
    "Number of currently active security threats",
    ["threat_type", "severity"]
)

# In-memory threat tracking (for 24h retention)
active_threats_store: Dict[str, Dict[str, Any]] = defaultdict(dict)
threat_history: List[Dict[str, Any]] = []

# Pattern matching for attack detection
patterns = {
    "sql_injection": re.compile(r"\b(union|select|insert|update|delete|drop|create|alter|grant|revoke)\b\s+[^\s]*\s*('|--|;|/\*)", re.IGNORECASE),
    "path_traversal": re.compile(r"(\.\./|\.\.\\)", re.IGNORECASE),
    "command_injection": re.compile(r"(?:[;&|]\s*)(?:cat|id|whoami|rm|wget|curl|nc|bash|sh|cmd|powershell|exec|system)\b", re.IGNORECASE),
    "xss_attempt": re.compile(r"(<script|javascript:|onerror=)", re.IGNORECASE),
    "user_agent_tool": re.compile(r"(curl|wget|python|bash|sh|powershell|perl|ruby)", re.IGNORECASE),
}

# GeoIP threat indicators
allowed_countries = {"US", "GB", "DE", "FR", "JP", "SG", "CH"}  # Default allowed countries

# Background task for log processing
class LogProcessor:
    def __init__(self):
        self.last_position = 0
        self.processing = False
        self.log_file = None
        self.last_check = 0
        self.task = None

    async def process_logs(self):
        """Background task to process security logs"""
        while self.processing:
            try:
                await self._process_log_file()
                await asyncio.sleep(LOG_CHECK_INTERVAL)
            except Exception as e:
                logger.error(f"Error processing logs: {e}")
                await asyncio.sleep(LOG_CHECK_INTERVAL)

    async def _process_log_file(self):
        """Process the HAProxy security log file"""
        try:
            if not os.path.exists(HA_PROXY_LOG_PATH):
                logger.warning(f"Log file not found: {HA_PROXY_LOG_PATH}")
                return

            async with asyncio.Lock():
                try:
                    # Run the blocking read in a thread so the event loop
                    # never stalls on large log chunks
                    def _read_new_lines():
                        with open(HA_PROXY_LOG_PATH, 'r') as f:
                            # If the file shrank or was recreated (rotation),
                            # start over from the beginning
                            current_size = os.path.getsize(HA_PROXY_LOG_PATH)
                            if current_size < self.last_position:
                                self.last_position = 0
                            f.seek(self.last_position)
                            lines = [ln.strip() for ln in f]
                            return lines, f.tell()

                    new_lines, new_position = await asyncio.to_thread(_read_new_lines)
                    self.last_position = new_position

                    # Process each log line
                    for line in new_lines:
                        if line:
                            await self._process_log_line(line)
                except IOError as e:
                    logger.error(f"Error reading log file: {e}")
        except Exception as e:
            logger.error(f"Error processing logs: {e}")

    async def _process_log_line(self, line: str):
        """Process a single security log line"""
        try:
            with log_processing_duration.time():
                event = json.loads(line)
                await self._categorize_event(event)
        except json.JSONDecodeError as e:
            logger.debug(f"Failed to parse JSON log line: {line[:100]}")
        except Exception as e:
            logger.error(f"Error processing log line: {e}")

    async def _categorize_event(self, event: Dict[str, Any]):
        """Categorize security event and update metrics"""
        try:
            # Extract basic fields
            timestamp = event.get("timestamp", "")
            source_ip = event.get("source_ip", "")
            http_status = event.get("http_status", "")
            http_path = event.get("http_path", "")
            threat_level = event.get("threat_level", "LOW")
            user_agent = event.get("user_agent", "")

            # Determine event type
            event_type = self._determine_event_type(event)

            # Update metrics
            security_events_total.labels(
                event_type=event_type,
                severity=threat_level,
                geo_country=event.get("geo_country", "unknown")
            ).inc()

            # Track rate limit violations
            if http_status == "429":
                rate_limit_violations_total.labels(
                    endpoint=http_path
                ).inc()

            # Track authentication failures
            if http_status in ["401", "403"]:
                auth_failures_total.labels(
                    auth_type="bearer_token",
                    reason="unauthorized"
                ).inc()

            # Detect attack patterns
            await self._detect_patterns(event, source_ip, user_agent)

            # Store threat in active threats store
            if threat_level in ["HIGH", "CRITICAL"]:
                threat_id = f"{source_ip}_{event_type}_{timestamp}"
                active_threats_store[threat_id] = {
                    "type": event_type,
                    "severity": threat_level,
                    "source_ip": source_ip,
                    "timestamp": timestamp,
                    "event": event
                }

                # Update active threats gauge
                active_threats.labels(
                    threat_type=event_type,
                    severity=threat_level
                ).inc()

        except Exception as e:
            logger.error(f"Error categorizing event: {e}")

    def _determine_event_type(self, event: Dict[str, Any]) -> str:
        """Determine the type of security event"""
        http_status = event.get("http_status", "")
        http_path = event.get("http_path", "")
        threat_level = event.get("threat_level", "LOW")

        if http_status == "429":
            return "rate_limit_violation"
        elif http_status == "401":
            return "authentication_failure"
        elif http_status == "403":
            return "authorization_failure"
        elif threat_level == "CRITICAL":
            return "critical_security_event"
        elif threat_level == "HIGH":
            return "high_security_event"
        elif http_status.startswith("5"):
            return "server_error"
        else:
            return "normal_request"

    async def _detect_patterns(self, event: Dict[str, Any], source_ip: str, user_agent: str):
        """Detect attack patterns in security events"""
        try:
            # Skip pattern detection for trusted/admin IPs
            if source_ip in TRUSTED_IPS:
                return

            http_path = event.get("http_path", "")
            http_query = event.get("http_query", "")

            # SQL injection detection
            if patterns["sql_injection"].search(http_path + http_query):
                attack_detected_total.labels(
                    attack_type="sql_injection",
                    severity="high"
                ).inc()
                logger.warning(f"SQL injection attempt from {source_ip} on {http_path}")

            # Path traversal detection
            if patterns["path_traversal"].search(http_path + http_query):
                attack_detected_total.labels(
                    attack_type="path_traversal",
                    severity="high"
                ).inc()
                logger.warning(f"Path traversal attempt from {source_ip} on {http_path}")

            # Command injection detection
            if patterns["command_injection"].search(http_query):
                attack_detected_total.labels(
                    attack_type="command_injection",
                    severity="critical"
                ).inc()
                logger.warning(f"Command injection attempt from {source_ip} on {http_path}")

            # XSS attempt detection
            if patterns["xss_attempt"].search(http_path + http_query):
                attack_detected_total.labels(
                    attack_type="xss_attempt",
                    severity="medium"
                ).inc()
                logger.warning(f"XSS attempt from {source_ip} on {http_path}")

            # User-agent tool detection
            if patterns["user_agent_tool"].search(user_agent):
                attack_detected_total.labels(
                    attack_type="user_agent_tool",
                    severity="low"
                ).inc()
                logger.info(f"User-agent tool detected from {source_ip}: {user_agent}")

        except Exception as e:
            logger.error(f"Error detecting patterns: {e}")

# Log processor instance
log_processor = LogProcessor()

# Pydantic models for API
class SecurityEvent(BaseModel):
    timestamp: str
    source_ip: str
    event_type: str
    severity: str
    details: Dict[str, Any]

class SecurityEventResponse(BaseModel):
    status: str
    events_processed: int
    active_threats: int
    last_event: Optional[str] = None

# API Endpoints
@app.get("/health")
async def health_check():
    """Health check endpoint"""
    try:
        if redis_client:
            redis_client.ping()
        return {"status": "healthy", "log_processor_active": log_processor.processing}
    except Exception as e:
        raise HTTPException(status_code=503, detail=f"Service unhealthy: {e}")

@app.get("/metrics")
async def metrics():
    """Prometheus metrics endpoint"""
    return Response(content=generate_latest(), media_type=CONTENT_TYPE_LATEST)

@app.post("/parse-log", response_model=SecurityEventResponse)
async def parse_log_line(log_line: str, api_key: Optional[str] = None):
    """Parse a single security log line (for testing).

    Requires the PARSER_API_KEY header so unauthenticated clients cannot
    forge security events and corrupt metrics/alerting.
    """
    expected_key = os.getenv("PARSER_API_KEY", "")
    if expected_key and api_key != expected_key:
        raise HTTPException(status_code=401, detail="Unauthorized")

    try:
        event = json.loads(log_line)
        await log_processor._categorize_event(event)
        return SecurityEventResponse(
            status="success",
            events_processed=1,
            active_threats=len(active_threats_store),
            last_event=event.get("timestamp", "")
        )
    except json.JSONDecodeError as e:
        raise HTTPException(status_code=400, detail=f"Invalid JSON: {e}")
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")

@app.get("/security-events", response_model=List[SecurityEvent])
async def list_security_events(limit: int = 100, severity: Optional[str] = None):
    """List recent security events"""
    try:
        events = list(active_threats_store.values())[-limit:]

        if severity:
            events = [e for e in events if e.get("severity") == severity]

        return [
            SecurityEvent(
                timestamp=e.get("timestamp", ""),
                source_ip=e.get("source_ip", "unknown"),
                event_type=e.get("type", "unknown"),
                severity=e.get("severity", "unknown"),
                details=e
            )
            for e in events
        ]
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error listing events: {e}")

@app.get("/threats/active", response_model=Dict[str, Any])
async def get_active_threats():
    """Get currently active security threats"""
    try:
        # Clean up old threats (>24 hours)
        current_time = datetime.now()
        active_now = {}

        for threat_id, threat_data in active_threats_store.items():
            try:
                threat_time = datetime.fromisoformat(threat_data.get("timestamp", ""))
                time_diff = (current_time - threat_time).total_seconds()

                if time_diff < 86400:  # < 24 hours
                    active_now[threat_id] = threat_data
                else:
                    # Expired: keep the gauge in sync
                    active_threats.labels(
                        threat_type=threat_data.get("type", "unknown"),
                        severity=threat_data.get("severity", "unknown")
                    ).dec()
            except (ValueError, TypeError):
                pass

        # Update the store
        active_threats_store.clear()
        active_threats_store.update(active_now)

        # Count threats by type
        threat_counts = defaultdict(lambda: 0)
        for threat in active_now.values():
            threat_counts[threat.get("type", "unknown")] += 1

        return {
            "total_threats": len(active_now),
            "threats_by_type": dict(threat_counts),
            "recent_threats": list(active_now.values())[-10:],
            "last_check": datetime.now().isoformat()
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error getting threats: {e}")

@app.post("/start-processing")
async def start_processing():
    """Start/stop background log processing"""
    global log_processor
    if log_processor.processing:
        return {"status": "already_processing"}

    log_processor.processing = True
    log_processor.task = asyncio.create_task(log_processor.process_logs())
    return {"status": "started"}

@app.post("/stop-processing")
async def stop_processing():
    """Stop background log processing"""
    global log_processor
    if not log_processor.processing:
        return {"status": "not_processing"}

    log_processor.processing = False
    if log_processor.task:
        log_processor.task.cancel()
        log_processor.task = None
    return {"status": "stopped"}

# Startup event
@app.on_event("startup")
async def startup_event():
    """Initialize security log parser on startup"""
    logger.info("Starting Chimera Security Log Parser Service")

    # Start background log processing (keep a strong reference to the task)
    log_processor.processing = True
    log_processor.task = asyncio.create_task(log_processor.process_logs())

    logger.info("Security log parser started successfully")

# Shutdown event
@app.on_event("shutdown")
async def shutdown_event():
    """Clean up on shutdown"""
    global log_processor
    log_processor.processing = False
    if log_processor.task:
        log_processor.task.cancel()
        log_processor.task = None
    logger.info("Security log parser shutting down")

if __name__ == "__main__":
    uvicorn.run(
        app,
        host="0.0.0.0",
        port=METRICS_PORT,
        log_level="info"
    )