#!/bin/bash
# SSL Certificate Generation Script for Chimera Trading System
# Supports both development (self-signed) and production (Let's Encrypt)

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
DOMAIN="${1:-chimera.example.com}"
EMAIL="${2:-admin@example.com}"
CERT_DIR="./ssl/certbot/letsencrypt"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "======================================================================"
echo "Chimera Trading System - SSL Certificate Generation"
echo "======================================================================"
echo "Domain: $DOMAIN"
echo "Email: $EMAIL"
echo "Certificate Directory: $CERT_DIR"
echo "======================================================================"

# Create directories
mkdir -p "$CERT_DIR"
mkdir -p "$SCRIPT_DIR/logs"
mkdir -p "$SCRIPT_DIR/work"

# Function to generate self-signed certificate for development
generate_self_signed() {
    echo -e "${YELLOW}Generating self-signed certificate for development...${NC}"

    # Check if openssl is available
    if ! command -v openssl &> /dev/null; then
        echo -e "${RED}Error: openssl is not installed${NC}"
        echo "Install openssl: brew install openssl"
        exit 1
    fi

    # Generate private key
    openssl genrsa -out "$CERT_DIR/chimera.key" 2048

    # Generate certificate
    openssl req -new -x509 -key "$CERT_DIR/chimera.key" \
        -out "$CERT_DIR/chimera.crt" -days 365 \
        -subj "/C=US/ST=State/L=City/O=Chimera/OU=Trading/CN=$DOMAIN"

    # Combine key and cert for HAProxy
    cat "$CERT_DIR/chimera.crt" "$CERT_DIR/chimera.key" > "$CERT_DIR/chimera.pem"

    echo -e "${GREEN}Self-signed certificate generated successfully${NC}"
    echo "Certificate: $CERT_DIR/chimera.pem"
    echo "Valid for: 365 days"
    echo ""
    echo "⚠️  WARNING: This is a self-signed certificate for development only!"
    echo "   Browsers will show security warnings. Use only for testing."
}

# Function to generate Let's Encrypt certificate for production
generate_lets_encrypt() {
    echo -e "${YELLOW}Requesting certificate from Let's Encrypt...${NC}"

    # Check if domain is the default example
    if [[ "$DOMAIN" == "chimera.example.com" ]]; then
        echo -e "${RED}Error: Cannot use example domain for Let's Encrypt${NC}"
        echo "Please provide a real domain: ./generate-certificates.sh yourdomain.com admin@yourdomain.com"
        exit 1
    fi

    # Check if docker is available
    if ! command -v docker &> /dev/null; then
        echo -e "${RED}Error: docker is not installed${NC}"
        echo "Install docker: https://docs.docker.com/get-docker/"
        exit 1
    fi

    # Run Certbot to obtain certificate
    echo "Running Certbot in standalone mode..."

    docker run --rm \
        -v "$SCRIPT_DIR/letsencrypt:/etc/letsencrypt" \
        -v "$SCRIPT_DIR/logs:/var/log/letsencrypt" \
        -v "$SCRIPT_DIR/work:/var/lib/letsencrypt" \
        -p 80:80 \
        certbot/certbot:latest certonly \
        --standalone \
        --email "$EMAIL" \
        --agree-tos \
        --no-eff-email \
        -d "$DOMAIN" || {
        echo -e "${RED}Certificate generation failed${NC}"
        echo "Please check:"
        echo "1. Domain DNS is correctly configured"
        echo "2. Port 80 is accessible from internet"
        echo "3. Domain is not already using certificates"
        exit 1
    }

    # Convert to combined PEM for HAProxy
    LIVE_DIR="/etc/letsencrypt/live/$DOMAIN"
    docker run --rm \
        -v "$SCRIPT_DIR/letsencrypt:/etc/letsencrypt" \
        -v "$CERT_DIR:/output" \
        alpine:latest sh -c \
        "cat $LIVE_DIR/fullchain.pem $LIVE_DIR/privkey.pem > /output/chimera.pem"

    echo -e "${GREEN}Certificate generated successfully${NC}"
    echo "Certificate: $CERT_DIR/chimera.pem"
    echo "Domain: $DOMAIN"
    echo ""
    echo "✅ Certificate is valid for 90 days and will need renewal"
}

# Main logic
if [[ "$DOMAIN" == "chimera.example.com" ]]; then
    # Default behavior - generate self-signed for development
    generate_self_signed
else
    # Check if user wants Let's Encrypt or self-signed
    echo ""
    echo "Select certificate type:"
    echo "1) Let's Encrypt (Production) - Requires public domain and port 80 access"
    echo "2) Self-Signed (Development) - For testing only"
    echo ""
    read -p "Enter choice [1-2]: " choice

    case $choice in
        1)
            generate_lets_encrypt
            ;;
        2)
            generate_self_signed
            ;;
        *)
            echo -e "${RED}Invalid choice. Generating self-signed certificate.${NC}"
            generate_self_signed
            ;;
    esac
fi

# Set proper permissions
chmod 600 "$CERT_DIR/chimera.key" 2>/dev/null || true
chmod 644 "$CERT_DIR/chimera.pem"

echo ""
echo -e "${GREEN}Certificate setup complete!${NC}"
echo ""
echo "Next steps:"
echo "1. Update docker-compose-haproxy.yml with your domain"
echo "2. Start HAProxy: docker-compose -f docker-compose-haproxy.yml up -d"
echo "3. Test HTTPS access: https://$DOMAIN"
echo ""
echo "For production, set up automatic renewal:"
echo "  - Add to crontab: 0 3 * * * $PWD/ssl/renew-certificates.sh"
echo "======================================================================"