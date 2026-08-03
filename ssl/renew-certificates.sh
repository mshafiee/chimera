#!/bin/bash
# SSL Certificate Renewal Script for Chimera Trading System
# Automatically renews Let's Encrypt certificates and reloads HAProxy

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
DOMAIN="${1:-chimera.example.com}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CERT_DIR="$SCRIPT_DIR/ssl/certbot/letsencrypt"
LOG_FILE="$SCRIPT_DIR/logs/renewal-$(date +%Y%m%d-%H%M%S).log"

echo "======================================================================"
echo "Chimera Trading System - SSL Certificate Renewal"
echo "======================================================================"
echo "Domain: $DOMAIN"
echo "Log File: $LOG_FILE"
echo "======================================================================"

# Check if certificate exists
if [[ ! -f "$CERT_DIR/chimera.pem" ]]; then
    echo -e "${RED}Error: Certificate not found at $CERT_DIR/chimera.pem${NC}"
    echo "Generate certificate first: ./ssl/generate-certificates.sh $DOMAIN"
    exit 1
fi

# Verify the existing certificate actually covers the requested domain
if ! openssl x509 -in "$CERT_DIR/chimera.pem" -noout -checkhost "$DOMAIN" > /dev/null 2>&1; then
    echo -e "${RED}Error: existing certificate at $CERT_DIR/chimera.pem does not cover $DOMAIN${NC}"
    exit 1
fi

# Check certificate expiry
CERT_EXPIRY=$(openssl x509 -enddate -noout -in "$CERT_DIR/chimera.pem" | cut -d= -f2)
CERT_EXPIRY_EPOCH=$(date -d "$CERT_EXPIRY" +%s)
CURRENT_EPOCH=$(date +%s)
DAYS_UNTIL_EXPIRY=$(( (CERT_EXPIRY_EPOCH - CURRENT_EPOCH) / 86400 ))

echo "Current certificate expires: $CERT_EXPIRY ($DAYS_UNTIL_EXPIRY days)"

# Only renew if certificate expires in less than 30 days
if [[ $DAYS_UNTIL_EXPIRY -gt 30 ]]; then
    echo -e "${GREEN}Certificate is still valid for $DAYS_UNTIL_EXPIRY days${NC}"
    echo "No renewal needed at this time."
    exit 0
fi

echo -e "${YELLOW}Certificate expires in $DAYS_UNTIL_EXPIRY days, attempting renewal...${NC}"

# Check if docker is available
if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: docker is not installed${NC}"
    exit 1
fi

# Certbot standalone needs port 80; stop HAProxy so the challenge can bind it.
HAPROXY_STOPPED=0
if docker ps -q --filter "name=^/chimera-haproxy$" | grep -q .; then
    echo "Stopping HAProxy to free port 80 for the ACME challenge..."
    docker stop chimera-haproxy
    HAPROXY_STOPPED=1
fi
# Always restart HAProxy (if it was running) when this script exits
trap 'if [ "${HAPROXY_STOPPED}" -eq 1 ]; then docker start chimera-haproxy > /dev/null 2>&1 || true; fi' EXIT

# Run Certbot renewal
echo "Running Certbot renewal..."

if docker run --rm \
    -v "$SCRIPT_DIR/letsencrypt:/etc/letsencrypt" \
    -v "$SCRIPT_DIR/logs:/var/log/letsencrypt" \
    -v "$SCRIPT_DIR/work:/var/lib/letsencrypt" \
    -p 80:80 \
    certbot/certbot:2.9.0 renew --force-renewal >> "$LOG_FILE" 2>&1; then

    echo -e "${GREEN}Certificate renewed successfully${NC}"

    # Convert to combined PEM for HAProxy: write to a temp file, validate it,
    # then atomically replace the old PEM so a failure never truncates it.
    LIVE_DIR="/etc/letsencrypt/live/$DOMAIN"
    TMP_PEM="$CERT_DIR/chimera.pem.tmp"
    rm -f "$TMP_PEM"
    if ! docker run --rm \
        -v "$SCRIPT_DIR/letsencrypt:/etc/letsencrypt" \
        -v "$CERT_DIR:/output" \
        alpine:3.19 sh -c \
        'cat "$1/fullchain.pem" "$1/privkey.pem" > /output/chimera.pem.tmp' \
        sh "$LIVE_DIR"; then
        echo -e "${RED}Failed to combine certificates into PEM${NC}" >&2
        exit 1
    fi

    if [ ! -s "$TMP_PEM" ] || ! openssl x509 -in "$TMP_PEM" -noout > /dev/null 2>&1; then
        echo -e "${RED}Combined PEM is empty or invalid; keeping previous certificate${NC}" >&2
        rm -f "$TMP_PEM"
        exit 1
    fi

    chmod 600 "$TMP_PEM"
    mv "$TMP_PEM" "$CERT_DIR/chimera.pem"

    echo -e "${GREEN}Certificate converted to PEM format${NC}"

    # Reload HAProxy if running
    if docker ps -q --filter "name=^/chimera-haproxy$" | grep -q .; then
        echo "Reloading HAProxy..."
        if ! docker kill -s HUP chimera-haproxy; then
            echo -e "${YELLOW}HAProxy reload failed - manual restart may be required${NC}"
            exit 1
        fi
        echo -e "${GREEN}HAProxy reloaded successfully${NC}"
    else
        echo -e "${YELLOW}HAProxy container not running - reload skipped${NC}"
    fi

    # Update renewal timestamp
    echo "Last renewed: $(date)" > "$SCRIPT_DIR/.last_renewal"

    echo ""
    echo -e "${GREEN}✅ Certificate renewal completed successfully${NC}"
    echo "New certificate expiry: $(openssl x509 -enddate -noout -in "$CERT_DIR/chimera.pem" | cut -d= -f2)"

else
    echo -e "${RED}Certificate renewal failed${NC}"
    echo "Check log file: $LOG_FILE"
    echo ""
    echo "Common issues:"
    echo "1. Domain DNS is not correctly configured"
    echo "2. Port 80 is blocked or not accessible"
    echo "3. Rate limiting by Let's Encrypt servers"
    echo "4. Invalid domain configuration"
    exit 1
fi

echo ""
echo "Next renewal check will be in 7 days"
echo "======================================================================"
