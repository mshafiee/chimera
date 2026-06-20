#!/bin/bash
set -euo pipefail

# Secret Rotation Initialization Script
#
# This script creates a baseline entry in the config_audit table to activate
# rotation tracking for fresh Chimera deployments.
#
# Usage: ./ops/initialize-secret-rotation.sh

# Chimera directory configuration
CHIMERA_DIR="${CHIMERA_DIR:-/opt/chimera}"
DB_PATH="${DB_PATH:-${CHIMERA_DIR}/data/chimera.db}"
CONFIG_FILE="${CONFIG_FILE:-${CHIMERA_DIR}/operator/.env}"

# Logging functions
log() {
    local level="$1"
    shift
    local message="$*"
    local timestamp
    timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[${timestamp}] [${level}] ${message}"
}

log_info() { log "INFO" "$@"; }
log_warn() { log "WARN" "$@"; }
log_error() { log "ERROR" "$@"; }

# Verify prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."

    # Check if database exists
    if [[ ! -f "$DB_PATH" ]]; then
        log_error "Database not found at: $DB_PATH"
        log_error "Please ensure Chimera is properly installed and the database exists."
        exit 1
    fi

    # Check if sqlite3 is available
    if ! command -v sqlite3 &> /dev/null; then
        log_error "sqlite3 command not found. Please install sqlite3."
        exit 1
    fi

    # Check if config_audit table exists
    local table_exists
    table_exists=$(sqlite3 "$DB_PATH" "SELECT name FROM sqlite_master WHERE type='table' AND name='config_audit';" 2>/dev/null || echo "")

    if [[ -z "$table_exists" ]]; then
        log_error "config_audit table not found in database."
        log_error "Please ensure the database schema is properly initialized."
        exit 1
    fi

    log_info "Prerequisites check passed."
}

# Check if rotation tracking is already initialized
check_already_initialized() {
    log_info "Checking if rotation tracking is already initialized..."

    local count
    count=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM config_audit WHERE key LIKE 'secret_rotation%';" 2>/dev/null || echo "0")

    if [[ "$count" -gt 0 ]]; then
        log_warn "Rotation tracking appears to be already initialized ($count entries found)."
        log_warn "If you want to reinitialize, please manually review the existing entries first."
        read -p "Continue anyway? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            log_info "Initialization cancelled."
            exit 0
        fi
    fi
}

# Initialize rotation tracking
initialize_rotation_tracking() {
    log_info "Initializing secret rotation tracking for fresh deployment..."

    local timestamp
    timestamp=$(date -u '+%Y-%m-%d %H:%M:%S')

    # Insert baseline entry to indicate tracking is active
    sqlite3 "$DB_PATH" "
    INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
    VALUES (
        'secret_rotation.initialized',
        'NULL',
        'tracking_active',
        'SYSTEM_INIT',
        'Secret rotation tracking initialized for fresh deployment'
    );
    " || {
        log_error "Failed to insert initialization entry into database."
        exit 1
    }

    log_info "Secret rotation tracking initialized successfully."
}

# Provide next steps guidance
show_next_steps() {
    log_info ""
    log_info "=========================================="
    log_info "Initialization completed successfully!"
    log_info "=========================================="
    log_info ""
    log_info "Next steps:"
    log_info "1. Perform your first manual rotation within 30 days:"
    log_info "   ./ops/rotate-secrets.sh webhook"
    log_info ""
    log_info "2. Setup automated rotation scheduling:"
    log_info "   ./ops/install-crons.sh"
    log_info ""
    log_info "3. Monitor rotation status via:"
    log_info "   - Web dashboard: /operations page"
    log_info "   - API: GET /api/v1/operations/secrets"
    log_info "   - Prometheus metrics: chimera_secret_rotation_initialized"
    log_info ""
    log_info "Current status:"
    log_info "- Rotation tracking: ACTIVE"
    log_info "- First rotation due: Within 30 days"
    log_info "- Automated scheduling: Not configured (run install-crons.sh)"
    log_info ""
}

# Main execution
main() {
    log_info "Starting secret rotation initialization..."
    log_info "Database: $DB_PATH"
    log_info ""

    check_prerequisites
    check_already_initialized
    initialize_rotation_tracking
    show_next_steps

    log_info "Done!"
}

# Run main function
main "$@"