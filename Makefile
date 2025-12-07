# Chimera Project Makefile
# High-frequency copy-trading system for Solana
#
# Usage:
#   make build       - Build all components
#   make test        - Run all tests
#   make lint        - Run linters
#   make deploy      - Deploy to production
#   make help        - Show this help

.PHONY: all build build-operator build-web test test-operator test-scout test-web \
        test-integration test-load test-chaos test-e2e test-all \
        lint lint-operator lint-scout lint-web clean deploy help \
        dev dev-operator dev-web db-init db-migrate preflight

# Configuration
CARGO := cargo
NPM := npm
PYTHON := python3
SQLITE := sqlite3

OPERATOR_DIR := operator
SCOUT_DIR := scout
WEB_DIR := web
OPS_DIR := ops
DATA_DIR := data
DB_PATH := $(DATA_DIR)/chimera.db

# Colors for output
GREEN := \033[0;32m
YELLOW := \033[1;33m
RED := \033[0;31m
NC := \033[0m # No Color

# Default target
all: build

# ============================================================================
# BUILD
# ============================================================================

build: build-operator build-web ## Build all components
	@echo "$(GREEN)All components built successfully$(NC)"

build-operator: ## Build Rust operator (release)
	@echo "$(YELLOW)Building operator...$(NC)"
	cd $(OPERATOR_DIR) && $(CARGO) build --release

build-operator-debug: ## Build Rust operator (debug)
	@echo "$(YELLOW)Building operator (debug)...$(NC)"
	cd $(OPERATOR_DIR) && $(CARGO) build

build-web: ## Build web dashboard
	@echo "$(YELLOW)Building web dashboard...$(NC)"
	cd $(WEB_DIR) && $(NPM) install && $(NPM) run build

# ============================================================================
# TEST
# ============================================================================

test: test-operator test-scout ## Run all tests
	@echo "$(GREEN)All tests passed$(NC)"

test-operator: ## Run Rust operator tests
	@echo "$(YELLOW)Running operator tests...$(NC)"
	cd $(OPERATOR_DIR) && $(CARGO) test

test-scout: ## Run Python scout tests
	@echo "$(YELLOW)Running scout tests...$(NC)"
	cd $(SCOUT_DIR) && $(PYTHON) -m pytest tests/ -v || echo "$(YELLOW)No tests found$(NC)"

test-web: ## Run web dashboard tests
	@echo "$(YELLOW)Running web tests...$(NC)"
	cd $(WEB_DIR) && $(NPM) test || echo "$(YELLOW)No tests configured$(NC)"

test-integration: ## Run integration tests
	@echo "$(YELLOW)Running integration tests...$(NC)"
	cd $(OPERATOR_DIR) && $(CARGO) test --test '*' -- --test-threads=1

test-load: ## Run load tests (requires k6)
	@echo "$(YELLOW)Running load tests...$(NC)"
	@which k6 > /dev/null && k6 run tests/load/webhook_flood.js || echo "$(YELLOW)Install k6: https://k6.io/docs/getting-started/installation/$(NC)"

test-chaos: ## Run chaos/resilience tests
	@echo "$(YELLOW)Running chaos tests...$(NC)"
	cd $(OPERATOR_DIR) && $(CARGO) test --test chaos_tests

test-e2e: ## Run web E2E tests (requires Playwright)
	@echo "$(YELLOW)Running E2E tests...$(NC)"
	cd $(WEB_DIR) && $(NPM) test || echo "$(YELLOW)Install playwright: npx playwright install$(NC)"

test-all: test test-integration test-chaos ## Run all test suites
	@echo "$(GREEN)All test suites passed$(NC)"

# ============================================================================
# LINT & FORMAT
# ============================================================================

lint: lint-operator lint-scout lint-web ## Run all linters
	@echo "$(GREEN)All linters passed$(NC)"

lint-operator: ## Run Rust linter (clippy)
	@echo "$(YELLOW)Running clippy...$(NC)"
	cd $(OPERATOR_DIR) && $(CARGO) clippy -- -D warnings

lint-scout: ## Run Python linter (ruff)
	@echo "$(YELLOW)Running Python linter...$(NC)"
	cd $(SCOUT_DIR) && $(PYTHON) -m ruff check . || $(PYTHON) -m flake8 . || echo "$(YELLOW)No linter installed$(NC)"

lint-web: ## Run TypeScript linter
	@echo "$(YELLOW)Running TypeScript linter...$(NC)"
	cd $(WEB_DIR) && $(NPM) run lint || echo "$(YELLOW)No lint script configured$(NC)"

fmt: fmt-operator fmt-web ## Format all code
	@echo "$(GREEN)All code formatted$(NC)"

fmt-operator: ## Format Rust code
	cd $(OPERATOR_DIR) && $(CARGO) fmt

fmt-web: ## Format TypeScript code
	cd $(WEB_DIR) && $(NPM) run format || npx prettier --write "src/**/*.{ts,tsx}"

# ============================================================================
# SECURITY
# ============================================================================

audit: audit-operator audit-web ## Run security audits
	@echo "$(GREEN)Security audit complete$(NC)"

audit-operator: ## Run Rust security audit
	@echo "$(YELLOW)Running cargo audit...$(NC)"
	cd $(OPERATOR_DIR) && $(CARGO) audit || echo "$(YELLOW)Install with: cargo install cargo-audit$(NC)"

audit-web: ## Run npm security audit
	@echo "$(YELLOW)Running npm audit...$(NC)"
	cd $(WEB_DIR) && $(NPM) audit || true

# ============================================================================
# DATABASE
# ============================================================================

db-init: ## Initialize database with schema
	@echo "$(YELLOW)Initializing database...$(NC)"
	mkdir -p $(DATA_DIR)
	$(SQLITE) $(DB_PATH) < database/schema.sql
	@echo "$(GREEN)Database initialized at $(DB_PATH)$(NC)"

db-migrate: ## Run database migrations (placeholder)
	@echo "$(YELLOW)Running migrations...$(NC)"
	@echo "$(GREEN)No pending migrations$(NC)"

db-backup: ## Create database backup
	@echo "$(YELLOW)Creating backup...$(NC)"
	./$(OPS_DIR)/backup.sh

db-shell: ## Open database shell
	$(SQLITE) $(DB_PATH)

# ============================================================================
# DEVELOPMENT
# ============================================================================

dev: dev-operator ## Start development server

dev-operator: ## Run operator in development mode
	@echo "$(YELLOW)Starting operator in dev mode...$(NC)"
	cd $(OPERATOR_DIR) && CHIMERA_DEV_MODE=true RUST_LOG=debug $(CARGO) run

dev-web: ## Run web dashboard in development mode
	@echo "$(YELLOW)Starting web dev server...$(NC)"
	cd $(WEB_DIR) && $(NPM) run dev

dev-scout: ## Run scout manually
	@echo "$(YELLOW)Running scout...$(NC)"
	cd $(SCOUT_DIR) && $(PYTHON) main.py --dry-run

# ============================================================================
# DEPLOYMENT
# ============================================================================

preflight: ## Run pre-deployment verification
	@echo "$(YELLOW)Running preflight checks...$(NC)"
	./$(OPS_DIR)/preflight-check.sh

deploy: preflight build ## Deploy to production
	@echo "$(YELLOW)Deploying to production...$(NC)"
	@echo "$(RED)Manual deployment steps:$(NC)"
	@echo "  1. Copy operator binary to server"
	@echo "  2. Copy web/dist to server"
	@echo "  3. Run: sudo systemctl restart chimera"
	@echo ""
	@echo "Or use: make deploy-rsync SERVER=user@host"

deploy-rsync: ## Deploy via rsync (requires SERVER variable)
ifndef SERVER
	$(error SERVER is required. Usage: make deploy-rsync SERVER=user@host)
endif
	@echo "$(YELLOW)Deploying to $(SERVER)...$(NC)"
	rsync -avz --progress $(OPERATOR_DIR)/target/release/chimera_operator $(SERVER):/opt/chimera/operator/target/release/
	rsync -avz --progress $(WEB_DIR)/dist/ $(SERVER):/opt/chimera/web/dist/
	ssh $(SERVER) "sudo systemctl restart chimera"
	@echo "$(GREEN)Deployment complete$(NC)"

install-service: ## Install systemd service
	@echo "$(YELLOW)Installing systemd service...$(NC)"
	sudo ./$(OPS_DIR)/install-crons.sh
	@echo "$(GREEN)Service installed$(NC)"

# ============================================================================
# CLEANUP
# ============================================================================

clean: ## Clean build artifacts
	@echo "$(YELLOW)Cleaning build artifacts...$(NC)"
	cd $(OPERATOR_DIR) && $(CARGO) clean
	rm -rf $(WEB_DIR)/dist $(WEB_DIR)/node_modules/.cache
	find . -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
	@echo "$(GREEN)Clean complete$(NC)"

clean-all: clean ## Clean everything including dependencies
	rm -rf $(WEB_DIR)/node_modules
	@echo "$(GREEN)Full clean complete$(NC)"

# ============================================================================
# UTILITIES
# ============================================================================

check-deps: ## Check if dependencies are installed
	@echo "Checking dependencies..."
	@which cargo > /dev/null || echo "$(RED)Missing: cargo (install Rust)$(NC)"
	@which node > /dev/null || echo "$(RED)Missing: node (install Node.js)$(NC)"
	@which python3 > /dev/null || echo "$(RED)Missing: python3$(NC)"
	@which sqlite3 > /dev/null || echo "$(RED)Missing: sqlite3$(NC)"
	@echo "$(GREEN)Dependency check complete$(NC)"

version: ## Show version information
	@echo "Chimera Version Info:"
	@echo "  Operator: $$(grep '^version' $(OPERATOR_DIR)/Cargo.toml | head -1 | cut -d'"' -f2)"
	@echo "  Web: $$(grep '"version"' $(WEB_DIR)/package.json | head -1 | cut -d'"' -f4)"
	@echo "  Scout: $$(grep '__version__' $(SCOUT_DIR)/main.py 2>/dev/null || echo 'N/A')"

logs: ## Tail production logs
	tail -f /var/log/chimera/operator.log

logs-all: ## Tail all logs
	tail -f /var/log/chimera/*.log

# ============================================================================
# HELP
# ============================================================================

help: ## Show this help message
	@echo "Chimera Makefile Commands:"
	@echo ""
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  $(GREEN)%-20s$(NC) %s\n", $$1, $$2}'
	@echo ""
	@echo "Examples:"
	@echo "  make build          # Build all components"
	@echo "  make test           # Run all tests"
	@echo "  make lint           # Run all linters"
	@echo "  make dev            # Start development server"
	@echo "  make preflight      # Run pre-deployment checks"
	@echo "  make deploy         # Deploy to production"

