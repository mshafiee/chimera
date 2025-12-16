#!/bin/bash
# Diagnose why trading is not happening

set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}=== Trading Activity Diagnosis ===${NC}\n"

# 1. Check system health
echo -e "${BLUE}1. System Health:${NC}"
HEALTH=$(curl -s http://localhost:8080/api/v1/health 2>/dev/null)
if echo "$HEALTH" | grep -q '"status".*"healthy"'; then
    echo -e "${GREEN}✓ Operator is healthy${NC}"
else
    echo -e "${RED}✗ Operator health check failed${NC}"
fi
echo "$HEALTH" | python3 -m json.tool 2>/dev/null | grep -E "(status|trading_allowed|circuit_breaker)" || echo "$HEALTH"

# 2. Check wallets
echo -e "\n${BLUE}2. Wallet Status:${NC}"
WALLETS=$(curl -s http://localhost:8080/api/v1/wallets 2>/dev/null)
TOTAL=$(echo "$WALLETS" | python3 -c "import sys, json; d=json.load(sys.stdin); print(d.get('total', 0))" 2>/dev/null || echo "0")
ACTIVE=$(echo "$WALLETS" | python3 -c "import sys, json; d=json.load(sys.stdin); wallets=d.get('wallets', []); print(sum(1 for w in wallets if w.get('status') == 'ACTIVE'))" 2>/dev/null || echo "0")
echo "Total wallets: $TOTAL"
echo "Active wallets: $ACTIVE"

if [ "$ACTIVE" -eq 0 ]; then
    echo -e "${RED}✗ No ACTIVE wallets - trading cannot start${NC}"
fi

# 3. Check monitoring status
echo -e "\n${BLUE}3. Monitoring Status:${NC}"
MONITORING=$(curl -s http://localhost:8080/api/v1/monitoring/status 2>/dev/null)
if [ -n "$MONITORING" ] && echo "$MONITORING" | grep -q "enabled"; then
    echo "$MONITORING" | python3 -m json.tool 2>/dev/null | head -20
else
    echo -e "${YELLOW}⚠ Monitoring endpoint not available or not configured${NC}"
    echo "This means wallets are not being monitored for trades"
fi

# 4. Check trades
echo -e "\n${BLUE}4. Trading Activity:${NC}"
TRADES=$(curl -s 'http://localhost:8080/api/v1/trades?limit=5' 2>/dev/null)
TRADE_COUNT=$(echo "$TRADES" | python3 -c "import sys, json; d=json.load(sys.stdin); print(d.get('total', 0))" 2>/dev/null || echo "0")
echo "Total trades: $TRADE_COUNT"

if [ "$TRADE_COUNT" -eq 0 ]; then
    echo -e "${YELLOW}⚠ No trades executed yet${NC}"
fi

# 5. Check operator logs for errors
echo -e "\n${BLUE}5. Recent Errors:${NC}"
ERRORS=$(docker logs chimera-operator --tail 50 2>&1 | grep -iE "ERROR" | tail -5)
if [ -n "$ERRORS" ]; then
    echo "$ERRORS"
else
    echo -e "${GREEN}✓ No recent errors${NC}"
fi

# 6. Check for monitoring configuration
echo -e "\n${BLUE}6. Configuration Check:${NC}"
if grep -q "helius_webhook_url" docker/env.mainnet-paper 2>/dev/null; then
    echo -e "${GREEN}✓ Helius webhook URL configured${NC}"
else
    echo -e "${YELLOW}⚠ Helius webhook URL not found in config${NC}"
    echo "  Monitoring requires: CHIMERA_MONITORING__HELIUS_WEBHOOK_URL"
fi

if grep -q "HELIUS_API_KEY" docker/env.mainnet-paper 2>/dev/null; then
    if grep -q "YOUR_HELIUS_API_KEY" docker/env.mainnet-paper 2>/dev/null; then
        echo -e "${RED}✗ Helius API key not configured (still has placeholder)${NC}"
    else
        echo -e "${GREEN}✓ Helius API key configured${NC}"
    fi
else
    echo -e "${YELLOW}⚠ Helius API key not found${NC}"
fi

# 7. Summary and recommendations
echo -e "\n${BLUE}=== Summary ===${NC}"
echo ""
if [ "$ACTIVE" -eq 0 ]; then
    echo -e "${RED}CRITICAL: No ACTIVE wallets${NC}"
    echo "  → Run scout to discover and promote wallets"
fi

if [ "$TRADE_COUNT" -eq 0 ]; then
    echo -e "${YELLOW}ISSUE: No trading activity${NC}"
    echo ""
    echo "Possible causes:"
    echo "  1. Monitoring not enabled for wallets"
    echo "  2. Helius webhook not configured"
    echo "  3. No trading signals received (wallets not trading)"
    echo "  4. Webhook URL not accessible from Helius"
    echo ""
    echo "Solutions:"
    echo "  1. Enable monitoring: POST /api/v1/monitoring/wallets/{address}/enable"
    echo "  2. Configure Helius webhook URL in docker/env.mainnet-paper"
    echo "  3. Ensure Helius API key is set"
    echo "  4. Check if wallets are actually trading on-chain"
fi



