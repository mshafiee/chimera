#!/bin/bash
# start-evaluation.sh - 10-Day evaluation orchestration script
# Coordinates the complete evaluation startup process with all services

set -euo pipefail

# ===================================================================
# CONFIGURATION
# ===================================================================
EVAL_DIR="${EVAL_DIR:-/opt/chimera/evaluation}"
COMPOSE_PROFILE="${1:-evaluation}"
DAY_NUM="${DAY_NUM:-1}"
SIGNAL_MODE="${SIGNAL_MODE:-replay}"

# Colors for terminal output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ===================================================================
# VALIDATION
# ===================================================================
echo "=========================================="
echo "Chimera 10-Day Evaluation Startup"
echo "=========================================="
echo "Evaluation Directory: ${EVAL_DIR}"
echo "Docker Profile: ${COMPOSE_PROFILE}"
echo "Signal Mode: ${SIGNAL_MODE}"
echo ""

# Check if running as root (required for Docker operations)
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Error: This script must be run as root for Docker operations${NC}"
    exit 1
fi

# Check if Docker is available
if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: Docker is not installed or not accessible${NC}"
    exit 1
fi

# Check if docker-compose is available
if ! command -v docker-compose &> /dev/null; then
    echo -e "${RED}Error: docker-compose is not installed${NC}"
    exit 1
fi

# ===================================================================
# ENVIRONMENT SETUP
# ===================================================================
echo -e "${BLUE}[1/8] Setting up evaluation environment...${NC}"

# Create evaluation directory structure
mkdir -p "${EVAL_DIR}"
mkdir -p "${EVAL_DIR}/signals"
mkdir -p "${EVAL_DIR}/profiles"
mkdir -p "${EVAL_DIR}/network-captures"
mkdir -p "${EVAL_DIR}/reports"
mkdir -p "${EVAL_DIR}/backup"

# Set permissions
chmod 755 "${EVAL_DIR}"

echo -e "${GREEN}✓ Evaluation directory structure created${NC}"

# ===================================================================
# DOCKER COMPOSE STARTUP
# ===================================================================
echo -e "${BLUE}[2/8] Starting evaluation Docker services...${NC}"

# Check if docker-compose files exist
if [ ! -f "docker-compose.yml" ]; then
    echo -e "${RED}Error: docker-compose.yml not found${NC}"
    exit 1
fi

if [ ! -f "docker-compose.evaluation.yml" ]; then
    echo -e "${RED}Error: docker-compose.evaluation.yml not found${NC}"
    exit 1
fi

# Start services with evaluation profile
docker-compose -f docker-compose.yml -f docker-compose.evaluation.yml --profile evaluation up -d

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓ Docker services started successfully${NC}"
else
    echo -e "${RED}✗ Failed to start Docker services${NC}"
    exit 1
fi

# ===================================================================
# SERVICE HEALTH CHECK
# ===================================================================
echo -e "${BLUE}[3/8] Checking service health...${NC}"

# Function to check service health
check_service_health() {
    local service_name=$1
    local health_url=$2
    local max_attempts=30
    local attempt=0

    while [ $attempt -lt $max_attempts ]; do
        if curl -sf "${health_url}" > /dev/null 2>&1; then
            echo -e "${GREEN}✓ ${service_name} is healthy${NC}"
            return 0
        fi

        attempt=$((attempt + 1))
        echo -e "${YELLOW}⏳ Waiting for ${service_name}... (${attempt}/${max_attempts})${NC}"
        sleep 2
    done

    echo -e "${RED}✗ ${service_name} failed health check${NC}"
    return 1
}

# Check critical services
check_service_health "Operator" "http://localhost:8080/api/v1/health"
check_service_health "Scout" "http://localhost:8081/health"
check_service_health "Prometheus Eval" "http://localhost:9091/-/healthy"

# ===================================================================
# EVALUATION DATABASE INITIALIZATION
# ===================================================================
echo -e "${BLUE}[4/8] Initializing evaluation database...${NC}"

# Wait for PostgreSQL to be ready
until docker-compose exec postgres-eval pg_isready -U chimera -d chimera_evaluation > /dev/null 2>&1; do
    echo -e "${YELLOW}⏳ Waiting for PostgreSQL to be ready...${NC}"
    sleep 2
done

echo -e "${GREEN}✓ PostgreSQL is ready${NC}"

# Create evaluation database schema if needed
docker-compose exec postgres-eval psql -U chimera -d chimera_evaluation -c "\dt" > /dev/null 2>&1
if [ $? -ne 0 ]; then
    echo -e "${YELLOW}⚠ Database schema not found, initializing...${NC}"
    docker-compose exec postgres-eval psql -U chimera -d chimera_evaluation -f /docker-entrypoint-initdb.d/01-schema.sql
else
    echo -e "${GREEN}✓ Evaluation database schema exists${NC}"
fi

# ===================================================================
# FLUENTD LOG AGGREGATION SETUP
# ===================================================================
echo -e "${BLUE}[5/8] Setting up log aggregation...${NC}"

# Check if Fluentd configuration exists
if [ -f "ops/fluentd/fluentd.conf" ]; then
    # Create log directories
    mkdir -p "${EVAL_DIR}/logs"
    mkdir -p "${EVAL_DIR}/logs/evaluation"

    # Restart Fluentd to load configuration
    docker-compose restart fluentd

    echo -e "${GREEN}✓ Fluentd log aggregation started${NC}"
else
    echo -e "${YELLOW}⚠ Fluentd configuration not found, skipping log aggregation${NC}"
fi

# ===================================================================
# DATA COLLECTION SETUP
# ===================================================================
echo -e "${BLUE}[6/8] Setting up automated data collection...${NC}"

# Make data collection script executable
chmod +x ops/collect-evaluation-data.sh

# Create hourly cron job for data collection
CRON_JOB="0 * * * * root ${EVAL_DIR}/ops/collect-evaluation-data.sh"

# Check if cron job already exists
if crontab -l 2>/dev/null | grep -q "collect-evaluation-data.sh"; then
    echo -e "${YELLOW}⚠ Data collection cron job already exists${NC}"
else
    # Add cron job
    (crontab -l 2>/dev/null; echo "${CRON_JOB}") | crontab -
    echo -e "${GREEN}✓ Data collection cron job added${NC}"
fi

# Create symbolic link for easy access
ln -sf "$(pwd)/ops/collect-evaluation-data.sh" "${EVAL_DIR}/collect-data.sh"

# ===================================================================
# SIGNAL PROCESSING SETUP
# ===================================================================
echo -e "${BLUE}[7/8] Setting up signal processing...${NC}"

if [ "${SIGNAL_MODE}" = "replay" ]; then
    # Historical signal replay mode (Days 1-5)
    echo -e "${GREEN}✓ Signal replay mode configured${NC}"

    # Check if historical signals file exists
    SIGNAL_FILE="${EVAL_DIR}/signals/historical_signals.jsonl"
    if [ ! -f "${SIGNAL_FILE}" ]; then
        echo -e "${YELLOW}⚠ Historical signals file not found: ${SIGNAL_FILE}${NC}"
        echo -e "${YELLOW}  Signals will need to be provided for historical replay${NC}"
    else
        echo -e "${GREEN}✓ Historical signals file found${NC}"
    fi

elif [ "${SIGNAL_MODE}" = "realtime" ]; then
    # Real-time signal recording mode (Days 6-10)
    echo -e "${GREEN}✓ Real-time signal recording mode configured${NC}"

    # Start signal collector
    python3 ops/signal-collector.py \
        --output-dir "${EVAL_DIR}/signals/realtime" \
        --duration-days 5 \
        --intercept-port 8090 &

    echo -e "${GREEN}✓ Signal collector started on port 8090${NC}"
else
    echo -e "${YELLOW}⚠ Unknown signal mode: ${SIGNAL_MODE}${NC}"
fi

# ===================================================================
# ANOMALY DETECTION STARTUP
# ===================================================================
echo -e "${BLUE}[8/8] Starting anomaly detection service...${NC}"

# Make anomaly detection script executable
chmod +x ops/detect-anomalies.py

# Start anomaly detector in background
python3 ops/detect-anomalies.py --interval 60 > "${EVAL_DIR}/anomaly-detection.log" 2>&1 &
ANOMALY_PID=$!

echo $ANOMALY_PID > "${EVAL_DIR}/anomaly-detector.pid"
echo -e "${GREEN}✓ Anomaly detection started (PID: ${ANOMALY_PID})${NC}"

# ===================================================================
# MONITORING DASHBOARD STARTUP
# ===================================================================
echo -e "${BLUE}[9/9] Starting monitoring dashboards...${NC}"

# Start Grafana and Prometheus check
docker-compose ps grafana-eval prometheus-eval > /dev/null 2>&1

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓ Monitoring dashboards are running${NC}"
    echo -e "${BLUE}  Grafana: http://localhost:3003${NC}"
    echo -e "${BLUE}  Prometheus Eval: http://localhost:9091${NC}"
else
    echo -e "${YELLOW}⚠ Some monitoring services may not be running${NC}"
fi

# ===================================================================
# INITIALIZATION COMPLETE
# ===================================================================
echo ""
echo "=========================================="
echo -e "${GREEN}🚀 Evaluation Startup Complete${NC}"
echo "=========================================="
echo ""
echo "Evaluation Configuration:"
echo "  Directory: ${EVAL_DIR}"
echo "  Signal Mode: ${SIGNAL_MODE}"
echo "  Starting Day: ${DAY_NUM}"
echo ""
echo "Service URLs:"
echo "  Operator Health: http://localhost:8080/api/v1/health"
echo "  Scout Health: http://localhost:8081/health"
echo "  Grafana Eval: http://localhost:3003"
echo "  Prometheus Eval: http://localhost:9091"
echo ""
echo "Data Collection:"
echo "  Hourly collection via cron"
echo "  Manual: ${EVAL_DIR}/collect-data.sh"
echo "  Logs: ${EVAL_DIR}/logs/evaluation/"
echo ""
echo "Monitoring:"
echo "  Anomaly Detection: Running (PID: ${ANOMALY_PID})"
echo "  Log Aggregation: Fluentd active"
echo ""
echo "Next Steps:"
echo "  1. Monitor service health: curl http://localhost:8080/api/v1/health"
echo "  2. View Grafana dashboards: http://localhost:3003"
echo "  3. Check anomaly detection logs: tail -f ${EVAL_DIR}/anomaly-detection.log"
echo "  4. Generate daily reports: ./ops/generate-daily-report.sh ${DAY_NUM}"
echo ""

# Send startup notification if Telegram is configured
if [ -n "${TELEGRAM_BOT_TOKEN:-}" ] && [ -n "${TELEGRAM_CHAT_ID:-}" ]; then
    STARTUP_MESSAGE="🚀 Chimera Evaluation Started
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
📅 Day: ${DAY_NUM}
🔧 Signal Mode: ${SIGNAL_MODE}
📍 Directory: ${EVAL_DIR}
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Services: Operator, Scout, Monitoring, Anomaly Detection
Status: All systems operational"

    curl -s "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
        -d "chat_id=${TELEGRAM_CHAT_ID}" \
        -d "text=${STARTUP_MESSAGE}" > /dev/null 2>&1

    echo -e "${GREEN}✓ Startup notification sent${NC}"
fi

# Create startup completion file
touch "${EVAL_DIR}/evaluation-started-${DAY_NUM}.txt"
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "${EVAL_DIR}/evaluation-started-${DAY_NUM}.txt"

echo "=========================================="
echo "Ready for 10-day evaluation!"
echo "=========================================="

exit 0