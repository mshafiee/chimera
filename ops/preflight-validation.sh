#!/bin/bash
# Comprehensive pre-flight validation for 10-day evaluation

echo "🚀 Chimera 10-Day Evaluation - Pre-Flight Validation"
echo "===================================================="
echo ""

PASSED=0
FAILED=0
WARNINGS=0

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

check_pass() {
    echo -e "${GREEN}✅ PASS${NC}: $1"
    ((PASSED++))
}

check_fail() {
    echo -e "${RED}❌ FAIL${NC}: $1"
    ((FAILED++))
}

check_warn() {
    echo -e "${YELLOW}⚠️  WARN${NC}: $1"
    ((WARNINGS++))
}

echo "📋 System Requirements"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check CPU cores (macOS/Linux compatible)
if [[ "$OSTYPE" == "darwin"* ]]; then
    CPU_CORES=$(sysctl -n hw.ncpu)
    MEMORY_GB=$(( $(sysctl -n hw.memsize) / 1024 / 1024 / 1024 ))
    DISK_GB=$(df -h . | awk 'NR==2 {print $4}' | tr -d 'GGMKi')
else
    CPU_CORES=$(nproc)
    MEMORY_GB=$(free -g | awk '/^Mem:/{print $2}')
    DISK_GB=$(df -BG . | awk 'NR==2 {print $4}' | tr -d 'G')
fi

if [ $CPU_CORES -ge 4 ]; then
    check_pass "CPU cores: $CPU_CORES (≥4 required)"
else
    check_fail "CPU cores: $CPU_CORES (<4 required)"
fi

# Check Memory
if [ $MEMORY_GB -ge 16 ]; then
    check_pass "Memory: ${MEMORY_GB}GB (≥16GB required)"
else
    check_fail "Memory: ${MEMORY_GB}GB (<16GB required)"
fi

# Check disk space
if [ $DISK_GB -ge 100 ]; then
    check_pass "Disk space: ${DISK_GB}GB free (≥100GB required)"
else
    check_fail "Disk space: ${DISK_GB}GB free (<100GB required)"
fi

echo ""
echo "🐳 Docker Environment"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check Docker
if command -v docker &> /dev/null; then
    DOCKER_VERSION=$(docker --version | awk '{print $3}' | tr -d ',')
    check_pass "Docker installed: $DOCKER_VERSION"
else
    check_fail "Docker not installed"
fi

# Check Docker Compose
if command -v docker-compose &> /dev/null; then
    COMPOSE_VERSION=$(docker-compose --version | awk '{print $4}' | tr -d ',')
    check_pass "Docker Compose installed: $COMPOSE_VERSION"
else
    check_fail "Docker Compose not installed"
fi

# Test Docker daemon
if docker ps &> /dev/null; then
    check_pass "Docker daemon running"
else
    check_fail "Docker daemon not running"
fi

echo ""
echo "📁 Evaluation Infrastructure"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check evaluation directory
if [ -d "evaluation" ]; then
    check_pass "Evaluation directory exists"
else
    check_fail "Evaluation directory missing"
fi

# Check required subdirectories
REQUIRED_DIRS=("signals" "anomalies" "logs" "profiles" "reports")
for dir in "${REQUIRED_DIRS[@]}"; do
    if [ -d "evaluation/$dir" ]; then
        check_pass "Subdirectory exists: evaluation/$dir"
    else
        check_fail "Subdirectory missing: evaluation/$dir"
    fi
done

echo ""
echo "📊 Historical Signals"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check historical signals file
SIGNALS_FILE="evaluation/signals/historical_signals.jsonl"
if [ -f "$SIGNALS_FILE" ]; then
    SIGNAL_COUNT=$(wc -l < "$SIGNALS_FILE")
    check_pass "Historical signals file: $SIGNAL_COUNT signals"

    # Validate JSONL format
    if python3 -c "
import sys
import json
errors = 0
with open('$SIGNALS_FILE') as f:
    for i, line in enumerate(f):
        try:
            signal = json.loads(line)
            required = ['timestamp', 'wallet_address', 'token_address', 'action', 'amount_sol', 'strategy']
            if not all(k in signal for k in required):
                print(f'Line {i+1}: Missing required fields')
                errors += 1
        except:
            errors += 1
sys.exit(0 if errors == 0 else 1)
"; then
        check_pass "Signals format validation: Valid JSONL"
    else
        check_fail "Signals format validation: Invalid JSONL"
    fi
else
    check_fail "Historical signals file missing"
fi

echo ""
echo "🔧 Configuration Files"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check evaluation environment
if [ -f "docker/env.evaluation" ]; then
    check_pass "Base evaluation environment exists"
else
    check_fail "Base evaluation environment missing"
fi

# Check local configuration
if [ -f "docker/env.evaluation.local" ]; then
    check_pass "Local evaluation environment exists"

    # Check for placeholder values
    if grep -q "your_helius_api_key_here" docker/env.evaluation.local; then
        check_warn "Helius API key still needs configuration"
    else
        check_pass "Helius API key configured"
    fi
else
    check_fail "Local evaluation environment missing"
fi

echo ""
echo "📜 Evaluation Scripts"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check required scripts
SCRIPTS=(
    "ops/start-evaluation.sh"
    "ops/monitor-evaluation.sh"
    "ops/signal-replayer.py"
    "ops/signal-collector.py"
    "ops/generate-daily-report.sh"
    "ops/generate-evaluation-report.py"
    "ops/collect-evaluation-data.sh"
)

for script in "${SCRIPTS[@]}"; do
    if [ -f "$script" ]; then
        if [ -x "$script" ] || [[ "$script" == *.py ]]; then
            check_pass "Script exists: $script"
        else
            check_warn "Script not executable: $script"
        fi
    else
        check_fail "Script missing: $script"
    fi
done

echo ""
echo "🗄️  Database Schema"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Check evaluation schema
if [ -f "database/evaluation_schema.sql" ]; then
    check_pass "Evaluation database schema exists"
else
    check_fail "Evaluation database schema missing"
fi

echo ""
echo "🌐 Network Connectivity"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Test network latency to Helius (if configured)
if grep -q "your_helius_api_key_here" docker/env.evaluation.local; then
    check_warn "Helius API key not configured - skipping network test"
else
    # Test basic connectivity
    if ping -c 1 -W 2 mainnet.helius-rpc.com &> /dev/null; then
        LATENCY=$(ping -c 1 mainnet.helius-rpc.com | awk '/time=/ {print $7}' | cut -d'=' -f2)
        if [ $(echo "$LATENCY < 50" | bc) -eq 1 ]; then
            check_pass "Network latency: ${LATENCY}ms (<50ms required)"
        else
            check_warn "Network latency: ${LATENCY}ms (above optimal but acceptable)"
        fi
    else
        check_fail "Cannot reach Helius RPC endpoint"
    fi
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Validation Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${GREEN}Passed: $PASSED${NC}"
echo -e "${YELLOW}Warnings: $WARNINGS${NC}"
echo -e "${RED}Failed: $FAILED${NC}"
echo ""

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}✅ PRE-FLIGHT VALIDATION PASSED${NC}"
    echo "You are ready to start the evaluation!"
    echo ""
    echo "Next steps:"
    echo "1. Configure Helius API key in docker/env.evaluation.local"
    echo "2. Run: sudo ./ops/start-evaluation.sh evaluation"
    exit 0
else
    echo -e "${RED}❌ PRE-FLIGHT VALIDATION FAILED${NC}"
    echo "Please resolve the failed checks before starting evaluation."
    exit 1
fi