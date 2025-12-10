#!/bin/bash
# Production Setup and Configuration Script
# Helps prepare Chimera for mainnet-prod deployment

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

log_error() {
    echo -e "${RED}[✗]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

check_file() {
    if [ -f "$1" ]; then
        log_success "$1 exists"
        return 0
    else
        log_error "$1 not found"
        return 1
    fi
}

log_section "Chimera Production Setup Checklist"

log_section "1. Secure Secrets Configuration"

# Check webhook secret
log_info "Checking webhook secret..."
if grep -q "REQUIRED_CHANGE_THIS" docker/env.mainnet-prod 2>/dev/null; then
    log_warning "Webhook secret needs to be changed in docker/env.mainnet-prod"
    log_info "Generate with: openssl rand -hex 32"
else
    log_success "Webhook secret appears configured"
fi

# Check Grafana password
log_info "Checking Grafana password..."
if grep -q "REQUIRED_CHANGE_THIS\|change-me" docker/env.mainnet-prod 2>/dev/null; then
    log_warning "Grafana password needs to be changed in docker/env.mainnet-prod"
else
    log_success "Grafana password appears configured"
fi

log_section "2. RPC Configuration"

# Check Helius API key
log_info "Checking RPC endpoints..."
if grep -q "YOUR_HELIUS_API_KEY" docker/env.mainnet-prod 2>/dev/null; then
    log_warning "Helius API key needs to be configured in docker/env.mainnet-prod"
    log_info "Get your key from: https://www.helius.dev/"
else
    log_success "RPC endpoints appear configured"
fi

log_section "3. Wallet Configuration"

# Check wallet private key
log_info "Checking wallet configuration..."
if grep -q "CHIMERA_WALLET__PRIVATE_KEY_ENCRYPTED=$" docker/env.mainnet-prod 2>/dev/null; then
    log_warning "Wallet private key needs to be encrypted and configured"
    log_info "Use the vault encryption system to store this securely"
else
    log_success "Wallet appears configured"
fi

log_section "4. Notifications Setup"

# Check Telegram
log_info "Checking Telegram notifications..."
if grep -q "your-telegram-bot-token\|^TELEGRAM_BOT_TOKEN=$" docker/env.mainnet-prod 2>/dev/null; then
    log_warning "Telegram notifications not configured (optional but recommended)"
    log_info "Get bot token from @BotFather on Telegram"
else
    log_success "Telegram notifications configured"
fi

# Check Discord
log_info "Checking Discord notifications..."
if grep -q "your-discord-webhook-url\|^DISCORD_WEBHOOK_URL=$" docker/env.mainnet-prod 2>/dev/null; then
    log_warning "Discord notifications not configured (optional)"
else
    log_success "Discord notifications configured"
fi

log_section "5. Circuit Breaker Settings"

log_info "Review circuit breaker thresholds:"
grep -A 4 "CIRCUIT_BREAKERS" docker/env.mainnet-prod | head -5
echo ""

log_section "6. Jito Configuration"

log_info "Jito settings:"
if grep -q "CHIMERA_JITO__ENABLED=true" docker/env.mainnet-prod; then
    log_success "Jito is enabled for MEV protection"
else
    log_warning "Jito is disabled - consider enabling for mainnet"
fi

log_section "7. Security Checklist"

echo "Before deploying to mainnet-prod, ensure:"
echo "  [ ] Webhook secret is strong and unique"
echo "  [ ] Grafana password is strong"
echo "  [ ] Wallet private key is encrypted"
echo "  [ ] RPC API keys are configured"
echo "  [ ] Admin wallets are configured in config.yaml"
echo "  [ ] Notifications are set up (recommended)"
echo "  [ ] Circuit breaker thresholds are appropriate"
echo "  [ ] Database backups are configured"
echo "  [ ] Monitoring alerts are configured"
echo ""

log_section "8. Pre-Deployment Testing"

echo "Recommended testing sequence:"
echo "  1. Test in devnet (current)"
echo "  2. Test in mainnet-paper (simulated trades)"
echo "  3. Review all metrics and logs"
echo "  4. Test circuit breaker trip/reset"
echo "  5. Test disaster recovery procedures"
echo "  6. Load test with realistic traffic"
echo ""

log_section "9. Deployment Commands"

echo "To deploy to mainnet-prod:"
echo ""
echo "  # 1. Review and update docker/env.mainnet-prod"
echo "  # 2. Initialize database:"
echo "     ./docker/docker-compose.sh init-db mainnet-prod"
echo ""
echo "  # 3. Start services:"
echo "     ./docker/docker-compose.sh start mainnet-prod"
echo ""
echo "  # 4. Monitor logs:"
echo "     docker logs -f chimera-operator"
echo ""
echo "  # 5. Check health:"
echo "     curl http://localhost:8080/api/v1/health"
echo ""

log_section "10. Post-Deployment Monitoring"

echo "Monitor these endpoints:"
echo "  - Health: http://localhost:8080/api/v1/health"
echo "  - Grafana: http://localhost:3002"
echo "  - Prometheus: http://localhost:9090"
echo "  - Web Dashboard: http://localhost:3000"
echo ""

log_info "Production setup checklist complete!"
log_warning "Review all items above before deploying to mainnet-prod"
