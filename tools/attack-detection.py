#!/usr/bin/env python3
"""
Chimera Attack Detection Service
Real-time attack pattern detection and threat analysis

This service:
1. Monitors security events for attack patterns
2. Detects brute force, DDoS, webhook floods, and injection attacks
3. Provides real-time threat assessment
4. Exposes metrics for Prometheus scraping
5. Generates alerts for critical threats
"""

from fastapi import FastAPI, HTTPException, BackgroundTasks, Response
from prometheus_client import Counter, Histogram, Gauge, generate_latest, CONTENT_TYPE_LATEST
from prometheus_fastapi_instrumentator import Instrumentator
from pydantic import BaseModel
from typing import Optional, Dict, Any, List
import redis
import uvicorn
import logging
from datetime import datetime, timedelta
import re
import json
from collections import defaultdict
from dataclasses import dataclass
import asyncio
import os

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Configuration
REDIS_HOST = os.getenv("REDIS_HOST", "localhost")
REDIS_PORT = int(os.getenv("REDIS_PORT", "6379"))
METRICS_PORT = int(os.getenv("METRICS_PORT", "8002"))
ALERT_THRESHOLD = int(os.getenv("ALERT_THRESHOLD", "10"))  # Default 10 events

# Detection thresholds
BRUTE_FORCE_THRESHOLD = 100  # failed auths per minute per IP
DDOS_MULTIPLIER = 10  # traffic multiplier for DDoS detection
WEBHOOK_FLOOD_THRESHOLD = 50  # webhook requests per second
DISTRIBUTED_BRUTE_FORCE_THRESHOLD = 1000  # total failed auths per minute

# FastAPI app
app = FastAPI(
    title="Chimera Attack Detection Service",
    description="Real-time attack pattern detection and threat analysis",
    version="1.0.0"
)

# Instrumentator for Prometheus metrics
instrumentator = Instrumentator(app)

# Redis connection for state tracking
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
    logger.warning(f"Redis connection failed: {e}. Running without persistent state.")
    redis_client = None

# Prometheus Metrics
# Attack detection counters
attacks_detected_total = Counter(
    "chimera_attacks_detected_total",
    "Total attacks detected by pattern type",
    labelnames=["attack_type", "severity", "source_ip"]
)

# Specific attack type metrics
brute_force_attempts_total = Counter(
    "chimera_brute_force_attempts_total",
    "Brute force authentication attempts detected",
    labelnames=["source_ip", "target"]
)

ddos_attacks_total = Counter(
    "chimera_ddos_attacks_total",
    "DDoS attacks detected",
    labelnames=["attack_vector", "severity"]
)

webhook_attacks_total = Counter(
    "chimera_webhook_attacks_total",
    "Webhook-specific attacks detected",
    labelnames=["attack_type", "severity"]
)

injection_attempts_total = Counter(
    "chimera_injection_attempts_total",
    "SQL injection and path traversal attempts",
    labelnames=["injection_type", "severity", "source_ip"]
)

# Active threat tracking
active_threats = Gauge(
    "chimera_active_threats",
    "Number of currently active security threats",
    labelnames=["threat_type", "severity"]
)

# Detection performance
detection_duration = Histogram(
    "chimera_attack_detection_duration_seconds",
    "Time taken to detect attack patterns"
)

# Attack pattern definitions
@dataclass
class AttackPattern:
    name: str
    pattern: re.Pattern
    severity: str
    description: str

attack_patterns = [
    AttackPattern(
        "sql_injection",
        re.compile(r"(union|select|insert|update|delete|drop|create|alter|grant|revoke)\s+", re.IGNORECASE),
        "high",
        "SQL injection attempt detected"
    ),
    AttackPattern(
        "path_traversal",
        re.compile(r"(\.\./|\.\.\\|%2e%2e|%252e%252e)", re.IGNORECASE),
        "high",
        "Path traversal attempt detected"
    ),
    AttackPattern(
        "command_injection",
        re.compile(r"(;|\||&|`|\$\()", re.IGNORECASE),
        "critical",
        "Command injection attempt detected"
    ),
    AttackPattern(
        "xss_attempt",
        re.compile(r"(<script|javascript:|onerror=|onload=|onclick=)", re.IGNORECASE),
        "medium",
        "XSS attempt detected"
    ),
    AttackPattern(
        "ldap_injection",
        re.compile(r"(\*\)|\(|\|\||&)", re.IGNORECASE),
        "medium",
        "LDAP injection attempt detected"
    ),
]

# Threat tracking state
active_threats_store: Dict[str, Dict[str, Any]] = defaultdict(dict)
threat_history: List[Dict[str, Any]] = []

class AttackDetector:
    """Main attack detection engine"""

    def __init__(self):
        self.processing = False
        self.attack_windows = defaultdict(lambda: defaultdict(list))  # IP -> attack_type -> timestamps
        self.baselines = defaultdict(list)  # endpoint -> request rates

    async def detect_brute_force(self, event: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Detect brute force authentication attacks"""
        try:
            source_ip = event.get("source_ip", "")
            http_status = event.get("http_status", "")
            timestamp = event.get("timestamp", datetime.now().isoformat())

            # Only track failed authentication attempts
            if http_status not in ["401", "403"]:
                return None

            # Add to attack window for this IP
            try:
                event_time = datetime.fromisoformat(timestamp)
            except:
                event_time = datetime.now()

            self.attack_windows[source_ip]["auth_failures"].append(event_time)

            # Clean old events (older than 1 minute)
            cutoff_time = event_time - timedelta(minutes=1)
            self.attack_windows[source_ip]["auth_failures"] = [
                t for t in self.attack_windows[source_ip]["auth_failures"]
                if t > cutoff_time
            ]

            # Check threshold
            recent_failures = len(self.attack_windows[source_ip]["auth_failures"])

            if recent_failures > BRUTE_FORCE_THRESHOLD:
                threat = {
                    "type": "brute_force",
                    "severity": "critical",
                    "source_ip": source_ip,
                    "detection_time": event_time.isoformat(),
                    "details": {
                        "attempts": recent_failures,
                        "duration": "60s",
                        "threshold": BRUTE_FORCE_THRESHOLD
                    }
                }

                await self._register_threat(threat)
                brute_force_attempts_total.labels(source_ip=source_ip, target="auth").inc()
                return threat

            return None

        except Exception as e:
            logger.error(f"Error detecting brute force: {e}")
            return None

    async def detect_ddos(self, event: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Detect DDoS patterns via traffic analysis"""
        try:
            source_ip = event.get("source_ip", "")
            endpoint = event.get("http_path", "")
            timestamp = event.get("timestamp", datetime.now().isoformat())

            # Track request rates per endpoint
            try:
                event_time = datetime.fromisoformat(timestamp)
            except:
                event_time = datetime.now()

            # Add to baseline tracking
            self.attack_windows[endpoint]["requests"].append(event_time)

            # Clean old requests (older than 1 minute)
            cutoff_time = event_time - timedelta(minutes=1)
            self.attack_windows[endpoint]["requests"] = [
                t for t in self.attack_windows[endpoint]["requests"]
                if t > cutoff_time
            ]

            current_rate = len(self.attack_windows[endpoint]["requests"])

            # Calculate baseline if we have enough data
            if len(self.baselines[endpoint]) > 10:
                baseline_rate = sum(self.baselines[endpoint]) / len(self.baselines[endpoint])

                # Detect traffic spike
                if current_rate > baseline_rate * DDOS_MULTIPLIER:
                    threat = {
                        "type": "ddos",
                        "severity": "critical",
                        "source_ip": source_ip,
                        "detection_time": event_time.isoformat(),
                        "details": {
                            "current_rate": current_rate,
                            "baseline_rate": baseline_rate,
                            "multiplier": current_rate / baseline_rate,
                            "endpoint": endpoint
                        }
                    }

                    await self._register_threat(threat)
                    ddos_attacks_total.labels(attack_vector="traffic_spike", severity="critical").inc()
                    return threat

            # Update baseline with current rate
            self.baselines[endpoint].append(current_rate)

            # Keep baseline manageable
            if len(self.baselines[endpoint]) > 100:
                self.baselines[endpoint] = self.baselines[endpoint][-50:]

            return None

        except Exception as e:
            logger.error(f"Error detecting DDoS: {e}")
            return None

    async def detect_webhook_attacks(self, event: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Detect webhook-specific attacks"""
        try:
            http_path = event.get("http_path", "")
            http_status = event.get("http_status", "")
            source_ip = event.get("source_ip", "")

            if not http_path.startswith("/api/v1/webhook"):
                return None

            timestamp = event.get("timestamp", datetime.now().isoformat())
            try:
                event_time = datetime.fromisoformat(timestamp)
            except:
                event_time = datetime.now()

            # Track 429 responses (rate limit violations)
            if http_status == "429":
                self.attack_windows[source_ip]["webhook_429s"].append(event_time)

                # Clean old events
                cutoff_time = event_time - timedelta(seconds=10)
                self.attack_windows[source_ip]["webhook_429s"] = [
                    t for t in self.attack_windows[source_ip]["webhook_429s"]
                    if t > cutoff_time
                ]

                recent_429s = len(self.attack_windows[source_ip]["webhook_429s"])

                if recent_429s > WEBHOOK_FLOOD_THRESHOLD:
                    threat = {
                        "type": "webhook_flood",
                        "severity": "high",
                        "source_ip": source_ip,
                        "detection_time": event_time.isoformat(),
                        "details": {
                            "rate_limit_violations": recent_429s,
                            "duration": "10s",
                            "threshold": WEBHOOK_FLOOD_THRESHOLD
                        }
                    }

                    await self._register_threat(threat)
                    webhook_attacks_total.labels(attack_type="flood", severity="high").inc()
                    return threat

            return None

        except Exception as e:
            logger.error(f"Error detecting webhook attacks: {e}")
            return None

    async def detect_injection_attacks(self, event: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Detect SQL injection, path traversal, and other injection attacks"""
        try:
            http_path = event.get("http_path", "")
            http_query = event.get("http_query", "")
            source_ip = event.get("source_ip", "")

            # Combine path and query for analysis
            request_content = f"{http_path} {http_query}"

            for pattern in attack_patterns:
                if pattern.pattern.search(request_content):
                    threat = {
                        "type": "injection",
                        "subtype": pattern.name,
                        "severity": pattern.severity,
                        "source_ip": source_ip,
                        "detection_time": datetime.now().isoformat(),
                        "details": {
                            "pattern_matched": pattern.name,
                            "description": pattern.description,
                            "request_sample": request_content[:200]
                        }
                    }

                    await self._register_threat(threat)
                    injection_attempts_total.labels(
                        injection_type=pattern.name,
                        severity=pattern.severity,
                        source_ip=source_ip
                    ).inc()
                    return threat

            return None

        except Exception as e:
            logger.error(f"Error detecting injection attacks: {e}")
            return None

    async def _register_threat(self, threat: Dict[str, Any]):
        """Register a detected threat"""
        try:
            threat_id = f"{threat['type']}_{threat['source_ip']}_{threat['detection_time']}"

            active_threats_store[threat_id] = threat

            # Update gauge
            active_threats.labels(
                threat_type=threat["type"],
                severity=threat["severity"]
            ).inc()

            # Add to history
            threat_history.append(threat)

            # Keep history manageable
            if len(threat_history) > 1000:
                threat_history[:] = threat_history[-500:]

            logger.warning(f"Threat registered: {threat['type']} from {threat['source_ip']}")

        except Exception as e:
            logger.error(f"Error registering threat: {e}")

    async def process_event(self, event: Dict[str, Any]) -> List[Dict[str, Any]]:
        """Process a security event for attack patterns"""
        detected_threats = []

        try:
            with detection_duration.seconds():
                # Run all detection methods
                brute_force = await self.detect_brute_force(event)
                if brute_force:
                    detected_threats.append(brute_force)

                ddos = await self.detect_ddos(event)
                if ddos:
                    detected_threats.append(ddos)

                webhook = await self.detect_webhook_attacks(event)
                if webhook:
                    detected_threats.append(webhook)

                injection = await self.detect_injection_attacks(event)
                if injection:
                    detected_threats.append(injection)

        except Exception as e:
            logger.error(f"Error processing event: {e}")

        return detected_threats

# Attack detector instance
attack_detector = AttackDetector()

# Pydantic models
class SecurityEvent(BaseModel):
    timestamp: str
    source_ip: str
    http_status: str
    http_path: str
    http_query: Optional[str] = None
    user_agent: Optional[str] = None
    threat_level: Optional[str] = None

class ThreatAlert(BaseModel):
    threat_id: str
    type: str
    severity: str
    source_ip: str
    detection_time: str
    details: Dict[str, Any]

class DetectionResponse(BaseModel):
    status: str
    threats_detected: int
    threats: List[ThreatAlert]

# API Endpoints
@app.get("/health")
async def health_check():
    """Health check endpoint"""
    try:
        if redis_client:
            redis_client.ping()
        return {"status": "healthy", "detector_active": attack_detector.processing}
    except Exception as e:
        raise HTTPException(status_code=503, detail=f"Service unhealthy: {e}")

@app.get("/metrics")
async def metrics():
    """Prometheus metrics endpoint"""
    return Response(content=generate_latest(), media_type=CONTENT_TYPE_LATEST)

@app.post("/detect", response_model=DetectionResponse)
async def detect_attacks(event: SecurityEvent):
    """Process a security event for attack detection"""
    try:
        threats = await attack_detector.process_event(event.dict())

        threat_alerts = [
            ThreatAlert(
                threat_id=f"{t['type']}_{t['source_ip']}_{t['detection_time']}",
                type=t["type"],
                severity=t["severity"],
                source_ip=t["source_ip"],
                detection_time=t["detection_time"],
                details=t.get("details", {})
            )
            for t in threats
        ]

        return DetectionResponse(
            status="success",
            threats_detected=len(threats),
            threats=threat_alerts
        )

    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Detection error: {e}")

@app.get("/threats/active")
async def get_active_threats():
    """Get currently active security threats"""
    try:
        # Clean up old threats (>1 hour)
        current_time = datetime.now()
        active_threats = {}

        for threat_id, threat_data in active_threats_store.items():
            try:
                threat_time = datetime.fromisoformat(threat_data.get("detection_time", ""))
                time_diff = (current_time - threat_time).total_seconds()

                if time_diff < 3600:  # < 1 hour
                    active_threats[threat_id] = threat_data
            except:
                pass

        # Update store
        active_threats_store.clear()
        active_threats_store.update(active_threats)

        # Count by type and severity
        threat_counts = defaultdict(lambda: defaultdict(int))
        for threat in active_threats.values():
            threat_counts[threat.get("type", "unknown")][threat.get("severity", "unknown")] += 1

        return {
            "total_threats": len(active_threats),
            "threats_by_type": {k: dict(v) for k, v in threat_counts.items()},
            "recent_threats": list(active_threats.values())[-20:],
            "last_update": current_time.isoformat()
        }

    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error getting threats: {e}")

@app.get("/threats/history")
async def get_threat_history(limit: int = 100):
    """Get threat history"""
    try:
        return {
            "total_threats": len(threat_history),
            "threats": threat_history[-limit:]
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error getting history: {e}")

@app.post("/threats/clear")
async def clear_threats():
    """Clear all active threats (use with caution)"""
    try:
        active_threats_store.clear()

        # Reset gauges
        for threat_type in ["brute_force", "ddos", "webhook_flood", "injection"]:
            for severity in ["low", "medium", "high", "critical"]:
                active_threats.labels(threat_type=threat_type, severity=severity).set(0)

        return {"status": "cleared", "threats_cleared": len(active_threats_store)}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error clearing threats: {e}")

# Startup event
@app.on_event("startup")
async def startup_event():
    """Initialize attack detection service on startup"""
    logger.info("Starting Chimera Attack Detection Service")
    logger.info(f"Detection thresholds - Brute Force: {BRUTE_FORCE_THRESHOLD}, DDoS Multiplier: {DDOS_MULTIPLIER}")
    logger.info("Attack detection service started successfully")

# Shutdown event
@app.on_event("shutdown")
async def shutdown_event():
    """Clean up on shutdown"""
    global attack_detector
    attack_detector.processing = False
    logger.info("Attack detection service shutting down")

if __name__ == "__main__":
    uvicorn.run(
        app,
        host="0.0.0.0",
        port=METRICS_PORT,
        log_level="info"
    )