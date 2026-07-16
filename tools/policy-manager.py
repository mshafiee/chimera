#!/usr/bin/env python3
"""
Chimera Access Control Policy Manager
Manages geographic and IP-based access control policies for HAProxy

This service:
1. Provides REST API for policy management
2. Validates policy configurations
3. Applies policies to HAProxy configuration
4. Logs all policy changes for audit trail
5. Supports real-time policy evaluation
"""

from fastapi import FastAPI, HTTPException, BackgroundTasks
from prometheus_client import Counter, Gauge, Histogram, generate_latest, CONTENT_TYPE_LATEST
from prometheus_fastapi_instrumentator import Instrumentator
from pydantic import BaseModel
from typing import Optional, Dict, Any, List
import yaml
import subprocess
import logging
import os
import json
import uvicorn
from datetime import datetime
import asyncio
import ipaddress
from pathlib import Path

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Configuration
POLICY_DIR = os.getenv("POLICY_DIR", "/Users/mohammad/Documents/GitHub/chimera/docker/haproxy/policies")
HAPROXY_CONFIG = os.getenv("HAPROXY_CONFIG", "/Users/mohammad/Documents/GitHub/chimera/docker/haproxy/haproxy.cfg")
HAPROXY_RELOAD_COMMAND = os.getenv("HAPROXY_RELOAD_COMMAND", "docker-compose restart haproxy")
METRICS_PORT = int(os.getenv("METRICS_PORT", "8003"))

# FastAPI app
app = FastAPI(
    title="Chimera Access Control Policy Manager",
    description="Manages geographic and IP-based access control policies",
    version="1.0.0"
)

# Instrumentator for Prometheus metrics
instrumentator = Instrumentator(app)

# Prometheus Metrics
policy_changes_total = Counter(
    "chimera_policy_changes_total",
    "Total policy changes made",
    labelnames=["action", "policy_type"]
)

policy_validations_total = Counter(
    "chimera_policy_validations_total",
    "Total policy validations performed",
    labelnames=["result"]
)

haproxy_reloads_total = Counter(
    "chimera_haproxy_reloads_total",
    "Total HAProxy configuration reloads",
    labelnames=["result"]
)

active_policies = Gauge(
    "chimera_active_policies",
    "Number of currently active policies",
    labelnames=["policy_type"]
)

policy_reload_duration = Histogram(
    "chimera_policy_reload_duration_seconds",
    "Time taken to reload HAProxy configuration"
)

# Pydantic models
class AccessPolicy(BaseModel):
    mode: str  # whitelist, blacklist, mixed, off
    allowed_countries: Optional[List[str]] = []
    blocked_countries: Optional[List[str]] = []
    allowed_ips: Optional[List[str]] = []
    blocked_ips: Optional[List[str]] = []
    allowed_cities: Optional[List[str]] = []
    blocked_cities: Optional[List[str]] = []
    allowed_asns: Optional[List[str]] = []
    blocked_asns: Optional[List[str]] = []

class EndpointPolicy(BaseModel):
    endpoint: str
    policy: AccessPolicy
    requires_authentication: Optional[bool] = False
    rate_limit_override: Optional[int] = None

class PolicyValidation(BaseModel):
    valid: bool
    errors: List[str] = []
    warnings: List[str] = []

class PolicyChangeResponse(BaseModel):
    status: str
    message: str
    policy_id: Optional[str] = None
    validation: Optional[PolicyValidation] = None

class AuditLogEntry(BaseModel):
    timestamp: str
    action: str
    policy_type: str
    changes: Dict[str, Any]
    performed_by: str
    validation_result: Dict[str, Any]

# Utility functions
def load_policy_config() -> Dict[str, Any]:
    """Load the main policy configuration file"""
    try:
        config_path = os.path.join(POLICY_DIR, "config.yaml")
        with open(config_path, 'r') as f:
            return yaml.safe_load(f)
    except Exception as e:
        logger.error(f"Error loading policy config: {e}")
        return {}

def save_policy_config(config: Dict[str, Any]) -> bool:
    """Save the main policy configuration file"""
    try:
        config_path = os.path.join(POLICY_DIR, "config.yaml")
        with open(config_path, 'w') as f:
            yaml.dump(config, f, default_flow_style=False)
        return True
    except Exception as e:
        logger.error(f"Error saving policy config: {e}")
        return False

def validate_ip_address(ip_str: str) -> bool:
    """Validate an IP address or CIDR range"""
    try:
        ipaddress.ip_network(ip_str, strict=False)
        return True
    except ValueError:
        return False

def validate_country_code(code: str) -> bool:
    """Validate ISO 3166-1 alpha-2 country code"""
    if len(code) != 2:
        return False
    return code.isalpha() and code.isupper()

def validate_asn(asn_str: str) -> bool:
    """Validate ASN number (format: AS12345 or 12345)"""
    try:
        if asn_str.startswith("AS"):
            number = int(asn_str[2:])
        else:
            number = int(asn_str)
        return 1 <= number <= 4294967295  # Valid ASN range
    except ValueError:
        return False

def validate_policy(policy: AccessPolicy) -> PolicyValidation:
    """Validate a policy configuration"""
    errors = []
    warnings = []

    # Validate mode
    if policy.mode not in ["whitelist", "blacklist", "mixed", "off"]:
        errors.append(f"Invalid mode: {policy.mode}")

    # Validate IP addresses
    for ip in policy.allowed_ips:
        if not validate_ip_address(ip):
            errors.append(f"Invalid allowed IP: {ip}")

    for ip in policy.blocked_ips:
        if not validate_ip_address(ip):
            errors.append(f"Invalid blocked IP: {ip}")

    # Validate country codes
    for country in policy.allowed_countries:
        if not validate_country_code(country):
            errors.append(f"Invalid allowed country code: {country}")

    for country in policy.blocked_countries:
        if not validate_country_code(country):
            errors.append(f"Invalid blocked country code: {country}")

    # Validate ASNs
    for asn in policy.allowed_asns:
        if not validate_asn(asn):
            errors.append(f"Invalid allowed ASN: {asn}")

    for asn in policy.blocked_asns:
        if not validate_asn(asn):
            errors.append(f"Invalid blocked ASN: {asn}")

    # Check for conflicts
    if set(policy.allowed_countries) & set(policy.blocked_countries):
        warnings.append("Some countries appear in both allowed and blocked lists")

    if set(policy.allowed_ips) & set(policy.blocked_ips):
        warnings.append("Some IPs appear in both allowed and blocked lists")

    return PolicyValidation(
        valid=len(errors) == 0,
        errors=errors,
        warnings=warnings
    )

def audit_log(action: str, policy_type: str, changes: Dict[str, Any], validation_result: Dict[str, Any]):
    """Create an audit log entry"""
    try:
        audit_entry = AuditLogEntry(
            timestamp=datetime.now().isoformat(),
            action=action,
            policy_type=policy_type,
            changes=changes,
            performed_by="system",  # Could be enhanced with authentication
            validation_result=validation_result
        )

        # Write to audit log file
        audit_path = os.path.join(POLICY_DIR, "audit.log")
        with open(audit_path, 'a') as f:
            f.write(json.dumps(audit_entry.dict()) + "\n")

        logger.info(f"Audit log: {action} on {policy_type}")
    except Exception as e:
        logger.error(f"Error writing audit log: {e}")

async def reload_haproxy() -> bool:
    """Reload HAProxy configuration"""
    try:
        with policy_reload_duration.seconds():
            # Execute HAProxy reload command
            result = subprocess.run(
                HAPROXY_RELOAD_COMMAND.split(),
                capture_output=True,
                text=True,
                timeout=30
            )

            if result.returncode == 0:
                haproxy_reloads_total.labels(result="success").inc()
                logger.info("HAProxy configuration reloaded successfully")
                return True
            else:
                haproxy_reloads_total.labels(result="failed").inc()
                logger.error(f"HAProxy reload failed: {result.stderr}")
                return False

    except Exception as e:
        haproxy_reloads_total.labels(result="error").inc()
        logger.error(f"Error reloading HAProxy: {e}")
        return False

# API Endpoints
@app.get("/health")
async def health_check():
    """Health check endpoint"""
    try:
        return {
            "status": "healthy",
            "policy_dir_exists": os.path.exists(POLICY_DIR),
            "config_file_exists": os.path.exists(os.path.join(POLICY_DIR, "config.yaml"))
        }
    except Exception as e:
        raise HTTPException(status_code=503, detail=f"Service unhealthy: {e}")

@app.get("/metrics")
async def metrics():
    """Prometheus metrics endpoint"""
    return Response(content=generate_latest(), media_type=CONTENT_TYPE_LATEST)

@app.get("/policies")
async def list_policies():
    """List all access control policies"""
    try:
        config = load_policy_config()
        return {
            "status": "success",
            "policies": config
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error loading policies: {e}")

@app.get("/policies/config")
async def get_policy_config():
    """Get the current policy configuration"""
    try:
        config = load_policy_config()
        return config
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error loading config: {e}")

@app.post("/policies/validate")
async def validate_policy(policy: AccessPolicy):
    """Validate a policy configuration"""
    try:
        validation = validate_policy(policy)
        policy_validations_total.labels(result="success" if validation.valid else "failed").inc()
        return validation
    except Exception as e:
        policy_validations_total.labels(result="error").inc()
        raise HTTPException(status_code=500, detail=f"Validation error: {e}")

@app.put("/policies/config")
async def update_policy_config(config: Dict[str, Any]):
    """Update the main policy configuration"""
    try:
        # Validate configuration structure
        if "access_control" not in config:
            raise HTTPException(status_code=400, detail="Invalid configuration: missing access_control section")

        # Save configuration
        if save_policy_config(config):
            policy_changes_total.labels(action="update", policy_type="main_config").inc()

            audit_log("update", "main_config", config, {"valid": True})

            return {
                "status": "success",
                "message": "Policy configuration updated successfully"
            }
        else:
            raise HTTPException(status_code=500, detail="Failed to save configuration")

    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error updating config: {e}")

@app.put("/policies/endpoint/{endpoint:path}")
async def update_endpoint_policy(endpoint: str, endpoint_policy: EndpointPolicy):
    """Update policy for a specific endpoint"""
    try:
        # Validate policy
        validation = validate_policy(endpoint_policy.policy)
        if not validation.valid:
            return PolicyChangeResponse(
                status="error",
                message="Policy validation failed",
                validation=validation
            )

        # Load current configuration
        config = load_policy_config()
        if "endpoint_policies" not in config["access_control"]:
            config["access_control"]["endpoint_policies"] = {}

        # Update endpoint policy
        config["access_control"]["endpoint_policies"][endpoint] = endpoint_policy.dict()

        # Save configuration
        if save_policy_config(config):
            policy_changes_total.labels(action="update", policy_type="endpoint").inc()

            audit_log("update", f"endpoint_{endpoint}", endpoint_policy.dict(), validation.dict())

            return PolicyChangeResponse(
                status="success",
                message=f"Endpoint policy updated for {endpoint}",
                policy_id=endpoint
            )
        else:
            raise HTTPException(status_code=500, detail="Failed to save configuration")

    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error updating endpoint policy: {e}")

@app.post("/policies/reload")
async def reload_policies(background_tasks: BackgroundTasks):
    """Apply policy changes by reloading HAProxy configuration"""
    try:
        # Trigger HAProxy reload in background
        background_tasks.add_task(reload_haproxy)

        return {
            "status": "submitted",
            "message": "HAProxy reload initiated"
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error initiating reload: {e}")

@app.get("/policies/whitelist/{list_type}")
async def get_whitelist(list_type: str):
    """Get whitelist entries by type (ips, countries, cities, asns)"""
    try:
        if list_type not in ["ips", "countries", "cities", "asns"]:
            raise HTTPException(status_code=400, detail="Invalid list type")

        file_path = os.path.join(POLICY_DIR, "whitelists", f"{list_type}.lst")
        if not os.path.exists(file_path):
            return {"entries": []}

        with open(file_path, 'r') as f:
            entries = [line.strip() for line in f if line.strip() and not line.startswith("#")]

        return {"entries": entries}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error reading whitelist: {e}")

@app.put("/policies/whitelist/{list_type}")
async def update_whitelist(list_type: str, entries: List[str]):
    """Update whitelist entries"""
    try:
        if list_type not in ["ips", "countries", "cities", "asns"]:
            raise HTTPException(status_code=400, detail="Invalid list type")

        # Validate entries based on type
        for entry in entries:
            if list_type == "ips":
                if not validate_ip_address(entry):
                    return {"status": "error", "message": f"Invalid IP: {entry}"}
            elif list_type == "countries":
                if not validate_country_code(entry):
                    return {"status": "error", "message": f"Invalid country code: {entry}"}
            elif list_type == "asns":
                if not validate_asn(entry):
                    return {"status": "error", "message": f"Invalid ASN: {entry}"}

        # Write to file
        file_path = os.path.join(POLICY_DIR, "whitelists", f"{list_type}.lst")
        with open(file_path, 'w') as f:
            f.write(f"# {list_type.capitalize()} Whitelist\n")
            f.write(f"# Last updated: {datetime.now().isoformat()}\n")
            for entry in entries:
                f.write(f"{entry}\n")

        policy_changes_total.labels(action="update", policy_type=f"whitelist_{list_type}").inc()

        return {"status": "success", "message": f"Whitelist updated for {list_type}"}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error updating whitelist: {e}")

@app.get("/policies/blacklist/{list_type}")
async def get_blacklist(list_type: str):
    """Get blacklist entries by type (ips, countries, cities, asns)"""
    try:
        if list_type not in ["ips", "countries", "cities", "asns"]:
            raise HTTPException(status_code=400, detail="Invalid list type")

        file_path = os.path.join(POLICY_DIR, "blacklists", f"{list_type}.lst")
        if not os.path.exists(file_path):
            return {"entries": []}

        with open(file_path, 'r') as f:
            entries = [line.strip() for line in f if line.strip() and not line.startswith("#")]

        return {"entries": entries}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error reading blacklist: {e}")

@app.put("/policies/blacklist/{list_type}")
async def update_blacklist(list_type: str, entries: List[str]):
    """Update blacklist entries"""
    try:
        if list_type not in ["ips", "countries", "cities", "asns"]:
            raise HTTPException(status_code=400, detail="Invalid list type")

        # Validate entries based on type
        for entry in entries:
            if list_type == "ips":
                if not validate_ip_address(entry):
                    return {"status": "error", "message": f"Invalid IP: {entry}"}
            elif list_type == "countries":
                if not validate_country_code(entry):
                    return {"status": "error", "message": f"Invalid country code: {entry}"}
            elif list_type == "asns":
                if not validate_asn(entry):
                    return {"status": "error", "message": f"Invalid ASN: {entry}"}

        # Write to file
        file_path = os.path.join(POLICY_DIR, "blacklists", f"{list_type}.lst")
        with open(file_path, 'w') as f:
            f.write(f"# {list_type.capitalize()} Blacklist\n")
            f.write(f"# Last updated: {datetime.now().isoformat()}\n")
            for entry in entries:
                f.write(f"{entry}\n")

        policy_changes_total.labels(action="update", policy_type=f"blacklist_{list_type}").inc()

        return {"status": "success", "message": f"Blacklist updated for {list_type}"}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error updating blacklist: {e}")

@app.get("/audit/log")
async def get_audit_log(limit: int = 100):
    """Get recent audit log entries"""
    try:
        audit_path = os.path.join(POLICY_DIR, "audit.log")
        if not os.path.exists(audit_path):
            return {"entries": []}

        entries = []
        with open(audit_path, 'r') as f:
            for line in f:
                try:
                    entry = json.loads(line.strip())
                    entries.append(entry)
                    if len(entries) >= limit:
                        break
                except json.JSONDecodeError:
                    continue

        return {"entries": entries}
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Error reading audit log: {e}")

# Startup event
@app.on_event("startup")
async def startup_event():
    """Initialize policy manager on startup"""
    logger.info("Starting Chimera Access Control Policy Manager")

    # Update active policies gauge
    try:
        config = load_policy_config()
        if config:
            active_policies.labels(policy_type="total").set(1)
            if "endpoint_policies" in config.get("access_control", {}):
                active_policies.labels(policy_type="endpoints").set(
                    len(config["access_control"]["endpoint_policies"])
                )
    except:
        pass

    logger.info("Policy manager started successfully")

# Shutdown event
@app.on_event("shutdown")
async def shutdown_event():
    """Clean up on shutdown"""
    logger.info("Policy manager shutting down")

if __name__ == "__main__":
    uvicorn.run(
        app,
        host="0.0.0.0",
        port=METRICS_PORT,
        log_level="info"
    )