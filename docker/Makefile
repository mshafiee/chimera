# Docker Compose Makefile Helper
# Provides convenient shortcuts for managing Docker Compose environments

.PHONY: help devnet-start devnet-stop devnet-logs devnet-shell \
        paper-start paper-stop paper-logs paper-shell \
        prod-start prod-stop prod-logs prod-shell \
        build-all clean-all init-all

# Default profile
PROFILE ?= devnet

help:
	@echo "Chimera Docker Compose Commands"
	@echo ""
	@echo "Devnet Commands:"
	@echo "  make devnet-start    - Start devnet services"
	@echo "  make devnet-stop     - Stop devnet services"
	@echo "  make devnet-logs     - View devnet logs"
	@echo "  make devnet-shell    - Access operator shell in devnet"
	@echo ""
	@echo "Mainnet Paper Trading Commands:"
	@echo "  make paper-start     - Start paper trading services"
	@echo "  make paper-stop      - Stop paper trading services"
	@echo "  make paper-logs      - View paper trading logs"
	@echo "  make paper-shell     - Access operator shell in paper trading"
	@echo ""
	@echo "Mainnet Production Commands:"
	@echo "  make prod-start      - Start production services"
	@echo "  make prod-stop       - Stop production services"
	@echo "  make prod-logs       - View production logs"
	@echo "  make prod-shell      - Access operator shell in production"
	@echo ""
	@echo "General Commands:"
	@echo "  make build-all       - Build all Docker images"
	@echo "  make init-all        - Initialize databases for all profiles"
	@echo "  make clean-all       - Clean all environments"

# Devnet commands
devnet-start:
	@./docker-compose.sh start devnet

devnet-stop:
	@./docker-compose.sh stop devnet

devnet-logs:
	@./docker-compose.sh logs devnet -f

devnet-shell:
	@./docker-compose.sh shell devnet operator

# Paper trading commands
paper-start:
	@./docker-compose.sh start mainnet-paper

paper-stop:
	@./docker-compose.sh stop mainnet-paper

paper-logs:
	@./docker-compose.sh logs mainnet-paper -f

paper-shell:
	@./docker-compose.sh shell mainnet-paper operator

# Production commands
prod-start:
	@./docker-compose.sh start mainnet-prod

prod-stop:
	@./docker-compose.sh stop mainnet-prod

prod-logs:
	@./docker-compose.sh logs mainnet-prod -f

prod-shell:
	@./docker-compose.sh shell mainnet-prod operator

# Build all images
build-all:
	@echo "Building images for all profiles..."
	@./docker-compose.sh build devnet
	@./docker-compose.sh build mainnet-paper
	@./docker-compose.sh build mainnet-prod

# Initialize all databases
init-all:
	@echo "Initializing databases..."
	@./docker-compose.sh init-db devnet
	@./docker-compose.sh init-db mainnet-paper
	@./docker-compose.sh init-db mainnet-prod

# Clean all environments
clean-all:
	@echo "Cleaning all environments..."
	@read -p "This will remove ALL containers, networks, and volumes. Continue? (y/N): " -n 1 -r; \
	echo; \
	if [[ $$REPLY =~ ^[Yy]$$ ]]; then \
		./docker-compose.sh clean devnet; \
		./docker-compose.sh clean mainnet-paper; \
		./docker-compose.sh clean mainnet-prod; \
	fi
