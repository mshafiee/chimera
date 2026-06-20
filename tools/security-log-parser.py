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

from fastapi import FastAPI, BackgroundTasks, HTTPException, Response
from prometheus_client import Counter, Histogram, Gauge, generate_latest, CONTENT_TYPE_LATEST
from prometheus_fastapi.instrumentator import Instrumentator
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
from typing import Tuple
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

# FastAPI app
app = FastAPI(
    title="Chimera Security Log Parser",
    description="Processes HAProxy security events and exposes metrics",
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
    ["event_type", "severity", "geo_country", "source_ip"],
    "Total security events processed from HAProxy"
)

rate_limit_violations_total = Counter(
    "chimera_haproxy_rate_limit_violations_total",
    ["endpoint", "source_ip"],
    "Rate limit violations detected by HAProxy"
)

auth_failures_total = Counter(
    "chimera_haproxy_auth_failures_total",
    ["auth_type", "source_ip", "reason"],
    "Authentication failures detected"
)

attack_detected_total = Counter(
    "chimera_haproxy_attack_detected_total",
    ["attack_type", "severity", "source_ip"],
    "Attack patterns detected by security analysis"
)

geo_anomalies_total = Counter(
    "chimera_haproxy_geo_anomalies_total",
    ["anomaly_type", "geo_country"],
    "Geographic access anomalies detected"
)

# Performance metrics
log_processing_duration = Histogram(
    "chimera_security_log_parser_processing_duration_seconds",
    "Time taken to process security log events"
)

active_threats = Gauge(
    "chimera_haproxy_active_threats",
    ["threat_type", "severity"],
    "Number of currently active security threats"
)

# In-memory threat tracking (for 24h retention)
active_threats_store: Dict[str, Dict[str, Any]] = defaultdict(dict)
threat_history: List[Dict[str, Any]] = []

# Pattern matching for attack detection
patterns = {
    "sql_injection": re.compile(r"(union|select|insert|update|delete|drop|create|alter|grant|revoke)\s+", re.IGNORECASE),
    "path_traversal": re.compile(r"(\.\./|\.\.\\)", re.IGNORECASE),
    "command_injection": re.compile(r"(;|\||&)", re.IGNORECASE),
    "xss_attempt": re.compile(r"(<script|javascript:|onerror=)", re.IGNORECASE),
    "user_agent_tool": re.compile(r"(curl|wget|python|bash|sh|powershell|perl|ruby)", re.IGNORECASE),
}

# GeoIP threat indicators
threat_countries = set()  # Can be configured via environment
allowed_countries = {"US", "GB", "DE", "FR", "JP", "SG", "CH"}  # Default allowed countries

# Background task for log processing
class LogProcessor:
    def __init__(self):
        self.last_position = 0
        self.processing = False
        self.log_file = None
        self.last_check = 0

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
                with open(HA_PROXY_LOG_PATH, 'r') as f:
                    # Seek to last position
                    f.seek(self.last_position)

                    # Read new lines
                    new_lines = []
                    for line in f:
                        new_lines.append(line.strip())

                    if new_lines:
                        self.last_position = f.tell()

                        # Process each log line
                        for line in new_lines:
                            if line:
                                await self._process_log_line(line)

            except IOError as e:
                logger.error(f"Error reading log file: {e}")

    async def _process_log_line(self, line: str):
        """Process a single security log line"""
        try:
            with log_processing_duration.seconds():
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
                geo_country=event.get("geo_country", "unknown"),
                source_ip=source_ip
            ).inc()

            # Track rate limit violations
            if http_status == "429":
                rate_limit_violations_total.labels(
                    endpoint=http_path,
                    source_ip=source_ip
                ).inc()

            # Track authentication failures
            if http_status in ["401", "403"]:
                auth_failures_total.labels(
                    auth_type="bearer_token",
                    source_ip=source_ip,
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
            http_path = event.get("http_path", "")
            http_query = event.get("http_query", "")

            # SQL injection detection
            if patterns["sql_injection"].search(http_path + http_query):
                attack_detected_total.labels(
                    attack_type="sql_injection",
                    severity="high",
                    source_ip=source_ip
                ).inc()
                logger.warning(f"SQL injection attempt from {source_ip} on {http_path}")

            # Path traversal detection
            if patterns["path_traversal"].search(http_path + http_query):
                attack_detected_total.labels(
                    attack_type="path_traversal",
                    severity="high",
                    source_ip=source_ip
                ).inc()
                logger.warning(f"Path traversal attempt from {source_ip} on {http_path}")

            # Command injection detection
            if patterns["command_injection"].search(http_query):
                attack_detected_total.labels(
                    attack_type="command_injection",
                    severity="critical",
                    source_ip=source_ip
                ).inc()
                logger.warning(f"Command injection attempt from {source_ip} on {http_path}")

            # XSS attempt detection
            if patterns["xss_attempt"].search(http_path + http_query):
                attack_detected_total.labels(
                    attack_type="xss_attempt",
                    severity="medium",
                    source_ip=source_ip
                ).inc()
                logger.warning(f"XSS attempt from {source_ip} on {http_path}")

            # User-agent tool detection
            if patterns["user_agent_tool"].search(user_agent):
                attack_detected_total.labels(
                    attack_type="user_agent_tool",
                    severity="low",
                    source_ip=source_ip
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
async def parse_log_line(log_line: str):
    """Parse a single security log line (for testing)"""
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
        active_threats = {}

        for threat_id, threat_data in active_threats_store.items():
            try:
                threat_time = datetime.fromisoformat(threat_data.get("timestamp", ""))
                time_diff = (current_time - threat_time).total_seconds()

                if time_diff < 86400:  # < 24 hours
                    active_threats[threat_id] = threat_data
            except:
                pass

        # Update gauge based on remaining threats
        active_threats_store.clear()
        active_threats_store.update(active_threats)

        # Update gauge for each threat type
        threat_counts = defaultdict(lambda: 0)
        for threat in active_threats.values():
            threat_counts[threat.get("type", "unknown")] += 1

        return {
            "total_threats": len(active_threats),
            "threats_by_type": dict(threat_counts),
            "recent_threats": list(active_threats.values())[-10:],
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
    asyncio.create_task(log_processor.process_logs())
    return {"status": "started"}

@app.post("/stop-processing")
async def stop_processing():
    """Stop background log processing"""
    global log_processor
    if not log_processor.processing:
        return {"status": "not_processing"}

    log_processor.processing = False
    return {"status": "stopped"}

# Startup event
@app.on_event("startup")
async def startup_event():
    """Initialize security log parser on startup"""
    logger.info("Starting Chimera Security Log Parser Service")

    # Start background log processing
    log_processor.processing = True
    asyncio.create_task(log_processor.process_logs())

    logger.info("Security log parser started successfully")

# Shutdown event
@app.on_event("shutdown")
async def shutdown_event():
    """Clean up on shutdown"""
    global log_processor
    log_processor.processing = False
    logger.info("Security log parser shutting down")

if __name__ == "__main__":
    uvicorn.run(
        "security_log_parser:app",
        host="0.0.0.0",
        port=METRICS_PORT,
        log_level="info"
    )