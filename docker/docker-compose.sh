#!/bin/bash
# Helper script for managing Docker Compose environments

set -e

PROFILES=("devnet" "mainnet-paper" "mainnet-prod")
DEFAULT_PROFILE="devnet"

usage() {
    cat << EOF
Usage: $0 <command> [profile] [options]

Commands:
    start       Start services for the specified profile
    stop        Stop services for the specified profile
    restart     Restart services for the specified profile
    logs        View logs (use -f for follow)
    status      Show service status
    build       Build Docker images
    shell       Access shell in a service (requires service name)
    exec        Execute command in a service (requires service name and command)
    init-db     Initialize database
    clean       Stop and remove containers, networks, and volumes
    help        Show this help message

Profiles:
    devnet          - Development/testing on Solana Devnet
    mainnet-paper   - Paper trading on Mainnet (simulated)
    mainnet-prod     - Production trading on Mainnet (REAL FUNDS)

Examples:
    $0 start devnet
    $0 logs mainnet-paper -f
    $0 shell devnet operator
    $0 exec devnet operator cargo test
    $0 init-db devnet
    $0 clean devnet

EOF
}

check_profile() {
    local profile=$1
    if [[ ! " ${PROFILES[@]} " =~ " ${profile} " ]]; then
        echo "Error: Invalid profile '$profile'"
        echo "Valid profiles: ${PROFILES[*]}"
        exit 1
    fi
}

check_env_file() {
    local profile=$1
    local script_dir="$(cd "$(dirname "$0")" && pwd)"
    local project_root="$(cd "$script_dir/.." && pwd)"
    local env_file="${project_root}/docker/env.${profile}"
    
    if [ ! -f "$env_file" ]; then
        echo "Warning: Environment file not found: $env_file"
        echo "Creating from template..."
        case $profile in
            devnet)
                cp "${project_root}/docker/env.devnet" "$env_file" 2>/dev/null || true
                ;;
            mainnet-paper)
                cp "${project_root}/docker/env.mainnet-paper" "$env_file" 2>/dev/null || true
                ;;
            mainnet-prod)
                cp "${project_root}/docker/env.mainnet-prod" "$env_file" 2>/dev/null || true
                ;;
        esac
        echo "Please edit $env_file with your configuration before starting services."
    fi
}

get_profile() {
    local profile=${1:-$DEFAULT_PROFILE}
    check_profile "$profile"
    echo "$profile"
}

compose_cmd() {
    local profile=$1
    shift
    export COMPOSE_PROFILE=$profile
    # Change to project root directory
    cd "$(dirname "$0")/.." || exit 1
    # Try docker compose (v2) first, fallback to docker-compose (v1)
    if command -v docker > /dev/null && docker compose version > /dev/null 2>&1; then
        docker compose --profile "$profile" "$@"
    elif command -v docker-compose > /dev/null; then
        docker-compose --profile "$profile" "$@"
    else
        echo "Error: Neither 'docker compose' nor 'docker-compose' found"
        exit 1
    fi
}

case "${1:-help}" in
    start)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        check_env_file "$PROFILE"
        echo "Starting Chimera services for profile: $PROFILE"
        compose_cmd "$PROFILE" up -d
        echo "Services started. Use '$0 logs $PROFILE' to view logs."
        ;;
    stop)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        echo "Stopping Chimera services for profile: $PROFILE"
        compose_cmd "$PROFILE" stop
        ;;
    restart)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        echo "Restarting Chimera services for profile: $PROFILE"
        compose_cmd "$PROFILE" restart
        ;;
    logs)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        shift 2 2>/dev/null || shift
        compose_cmd "$PROFILE" logs "$@"
        ;;
    status)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        compose_cmd "$PROFILE" ps
        ;;
    build)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        echo "Building Docker images for profile: $PROFILE"
        compose_cmd "$PROFILE" build "$@"
        ;;
    shell)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        SERVICE=${3:-operator}
        echo "Accessing shell in $SERVICE (profile: $PROFILE)"
        compose_cmd "$PROFILE" exec "$SERVICE" /bin/sh || compose_cmd "$PROFILE" exec "$SERVICE" /bin/bash
        ;;
    exec)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        SERVICE=${3:-operator}
        shift 3
        if [ $# -eq 0 ]; then
            echo "Error: Command required"
            echo "Usage: $0 exec <profile> <service> <command>"
            exit 1
        fi
        compose_cmd "$PROFILE" exec "$SERVICE" "$@"
        ;;
    init-db)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        echo "Initializing database for profile: $PROFILE"
        SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
        PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
        cd "$PROJECT_ROOT" || exit 1
        if [ -f "docker/init-db.sh" ]; then
            bash docker/init-db.sh
        else
            mkdir -p data
            if [ -f "database/schema.sql" ]; then
                sqlite3 data/chimera.db < database/schema.sql
                echo "✓ Database initialized"
            else
                echo "✗ Error: database/schema.sql not found"
                exit 1
            fi
        fi
        ;;
    clean)
        PROFILE=$(get_profile "${2:-$DEFAULT_PROFILE}")
        read -p "This will remove all containers, networks, and volumes for $PROFILE. Continue? (y/N): " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            compose_cmd "$PROFILE" down -v
            echo "Cleaned up resources for $PROFILE"
        else
            echo "Cancelled"
        fi
        ;;
    help|--help|-h)
        usage
        ;;
    *)
        echo "Error: Unknown command '$1'"
        echo
        usage
        exit 1
        ;;
esac
