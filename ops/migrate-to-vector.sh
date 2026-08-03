#!/bin/bash
# Vector Migration Script
# Migrates from Fluentd to Vector for log aggregation

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

BACKUP_DATE=$(date +%Y%m%d-%H%M%S)

echo -e "${BLUE}=========================================="
echo "Vector Migration for Chimera Evaluation"
echo -e "==========================================${NC}"
echo ""

# Pre-flight checks
echo -e "${YELLOW}[Preflight Checks]${NC}"

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}✗ Please run as root${NC}"
    exit 1
fi

# Check if Vector configuration exists
if [ ! -f "ops/vector/vector.toml" ]; then
    echo -e "${RED}✗ Vector configuration not found${NC}"
    echo "Create the configuration first with ops/vector/vector.toml"
    exit 1
fi
echo -e "${GREEN}✓ Vector configuration found${NC}"

# Check if Docker Compose file exists
if [ ! -f "docker-compose.evaluation.yml" ]; then
    echo -e "${RED}✗ Docker Compose evaluation file not found${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Docker Compose file found${NC}"

# Step 1: Backup existing configuration
echo ""
echo -e "${YELLOW}[1/5] Backing up existing configuration...${NC}"

cp docker-compose.evaluation.yml docker-compose.evaluation.yml.backup-${BACKUP_DATE}
echo -e "${GREEN}✓ Docker Compose backed up${NC}"

ENV_BACKED_UP=0
if [ -f "docker/env.evaluation" ]; then
    cp docker/env.evaluation docker/env.evaluation.backup-${BACKUP_DATE}
    ENV_BACKED_UP=1
    echo -e "${GREEN}✓ Environment variables backed up${NC}"
fi

# Step 2: Deploy Vector configuration
echo ""
echo -e "${YELLOW}[2/5] Deploying Vector configuration...${NC}"

mkdir -p ops/vector
if [ -f "ops/vector/vector.toml" ]; then
    echo -e "${GREEN}✓ Vector configuration exists${NC}"
else
    echo -e "${RED}✗ Vector configuration not found${NC}"
    exit 1
fi

# Step 3: Start Vector (before removing Fluentd, keeping it as fallback)
echo ""
echo -e "${YELLOW}[3/5] Starting Vector...${NC}"

if docker-compose -f docker-compose.evaluation.yml --profile evaluation up -d vector; then
    echo -e "${GREEN}✓ Vector started${NC}"
else
    echo -e "${RED}✗ Vector failed to start${NC}"
    echo "Check logs with: docker-compose logs vector"
    exit 1
fi

# Wait for Vector to be healthy (poll, up to 60s)
echo -e "${YELLOW}[4/5] Waiting for Vector to be healthy...${NC}"
for i in $(seq 1 30); do
    if curl -sf --max-time 3 http://localhost:8383/health > /dev/null 2>&1; then
        echo -e "${GREEN}✓ Vector health check passed${NC}"
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo -e "${RED}✗ Vector did not become healthy in time${NC}"
        echo "Check logs with: docker-compose logs vector"
        exit 1
    fi
    sleep 2
done

# Check metrics endpoint
if curl -sf --max-time 3 http://localhost:8383/metrics > /dev/null 2>&1; then
    echo -e "${GREEN}✓ Vector metrics endpoint available${NC}"
else
    echo -e "${YELLOW}⚠ Vector metrics endpoint not available${NC}"
fi

# Check container status (exact name match)
if [ -n "$(docker ps -q --filter name=^/chimera-vector$)" ]; then
    echo -e "${GREEN}✓ Vector container running${NC}"
else
    echo -e "${RED}✗ Vector container not running${NC}"
    exit 1
fi

# Step 5: Stop and remove legacy Fluentd (only now that Vector is verified)
echo ""
echo -e "${YELLOW}[5/5] Removing legacy Fluentd...${NC}"

if [ -n "$(docker ps -aq --filter name=^/chimera-fluentd$)" ]; then
    docker rm -f chimera-fluentd
    echo -e "${GREEN}✓ Legacy Fluentd removed${NC}"
else
    echo -e "${YELLOW}⚠ No legacy Fluentd container running${NC}"
fi

# Create log directories
mkdir -p evaluation/logs/evaluation
echo -e "${GREEN}✓ Log directories created${NC}"

echo ""
echo -e "${GREEN}=========================================="
echo "Migration Complete!"
echo -e "==========================================${NC}"
echo ""
echo "Vector is now running and ready for log aggregation."
echo ""
echo "Next Steps:"
echo "  1. Monitor log collection: ls -la evaluation/logs/evaluation/"
echo "  2. Check metrics: curl http://localhost:8383/metrics"
echo "  3. Monitor Vector: ./ops/monitor-vector.sh"
echo "  4. Run integration tests: ./ops/test-vector-integration.sh"
echo ""
echo "Backup files created:"
echo "  - docker-compose.evaluation.yml.backup-${BACKUP_DATE}"
if [ "$ENV_BACKED_UP" -eq 1 ]; then
    echo "  - docker/env.evaluation.backup-${BACKUP_DATE}"
fi
echo ""
echo "🎉 Migration successful! You're now using Vector for log aggregation."
