#!/bin/bash
# Chimera Operations Installation Script
#
# Installs:
# - Systemd service
# - Cron jobs (backup, reconciliation)
# - Log rotation configuration
# - Creates required directories and users
#
# Usage: sudo ./install-crons.sh [--uninstall]

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
CHIMERA_USER="chimera"
CHIMERA_GROUP="chimera"
OPS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() { echo -e "${GREEN}[INFO]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

# Check if running as root
check_root() {
    if [[ $EUID -ne 0 ]]; then
        log_error "This script must be run as root (use sudo)"
        exit 1
    fi
}

# Create chimera user and group
create_user() {
    if ! getent group "$CHIMERA_GROUP" > /dev/null 2>&1; then
        log_info "Creating group: $CHIMERA_GROUP"
        groupadd --system "$CHIMERA_GROUP"
    fi
    
    if ! getent passwd "$CHIMERA_USER" > /dev/null 2>&1; then
        log_info "Creating user: $CHIMERA_USER"
        useradd --system --gid "$CHIMERA_GROUP" --home-dir "$CHIMERA_HOME" \
            --shell /usr/sbin/nologin "$CHIMERA_USER"
    fi
}

# Create required directories
create_directories() {
    log_info "Creating directories"
    
    mkdir -p "$CHIMERA_HOME"/{data,backups,config}
    mkdir -p /var/log/chimera
    
    chown -R "$CHIMERA_USER:$CHIMERA_GROUP" "$CHIMERA_HOME"
    chown -R "$CHIMERA_USER:$CHIMERA_GROUP" /var/log/chimera
    
    chmod 750 "$CHIMERA_HOME"
    chmod 750 /var/log/chimera
}

# Install systemd service
install_systemd() {
    log_info "Installing systemd service"
    
    cp "$OPS_DIR/chimera.service" /etc/systemd/system/chimera.service
    chmod 644 /etc/systemd/system/chimera.service
    
    systemctl daemon-reload
    systemctl enable chimera.service
    
    log_info "Systemd service installed (run 'systemctl start chimera' to start)"
}

# Install cron jobs
install_crons() {
    log_info "Installing cron jobs"
    
    # Make scripts executable
    chmod +x "$OPS_DIR/backup.sh"
    chmod +x "$OPS_DIR/reconcile.sh"
    chmod +x "$OPS_DIR/rotate-secrets.sh" 2>/dev/null || true
    chmod +x "$OPS_DIR/preflight-check.sh" 2>/dev/null || true
    chmod +x "$OPS_DIR/generate-reports.sh" 2>/dev/null || true
    chmod +x "$OPS_DIR/update-metrics.sh" 2>/dev/null || true
    
    # Copy scripts to /opt/chimera/ops
    mkdir -p "$CHIMERA_HOME/ops"
    cp "$OPS_DIR/backup.sh" "$CHIMERA_HOME/ops/"
    cp "$OPS_DIR/reconcile.sh" "$CHIMERA_HOME/ops/"
    [[ -f "$OPS_DIR/rotate-secrets.sh" ]] && cp "$OPS_DIR/rotate-secrets.sh" "$CHIMERA_HOME/ops/"
    [[ -f "$OPS_DIR/preflight-check.sh" ]] && cp "$OPS_DIR/preflight-check.sh" "$CHIMERA_HOME/ops/"
    [[ -f "$OPS_DIR/generate-reports.sh" ]] && cp "$OPS_DIR/generate-reports.sh" "$CHIMERA_HOME/ops/"
    [[ -f "$OPS_DIR/update-metrics.sh" ]] && cp "$OPS_DIR/update-metrics.sh" "$CHIMERA_HOME/ops/"
    chown -R "$CHIMERA_USER:$CHIMERA_GROUP" "$CHIMERA_HOME/ops"
    
    # Create crontab entries
    local cron_file="/etc/cron.d/chimera"
    local cron_content=""
    
    cron_content+="# Chimera scheduled tasks\n"
    cron_content+="# Managed by install-crons.sh - do not edit manually\n\n"
    cron_content+="SHELL=/bin/bash\n"
    cron_content+="PATH=/usr/local/bin:/usr/bin:/bin\n"
    cron_content+="CHIMERA_HOME=$CHIMERA_HOME\n"
    cron_content+="MAILTO=\"\"\n\n"
    
    cron_content+="# Daily backup at 3:00 AM\n"
    cron_content+="0 3 * * * $CHIMERA_USER $CHIMERA_HOME/ops/backup.sh >> /var/log/chimera/backup.log 2>&1\n\n"
    
    cron_content+="# Daily reconciliation at 4:00 AM\n"
    cron_content+="0 4 * * * $CHIMERA_USER $CHIMERA_HOME/ops/reconcile.sh >> /var/log/chimera/reconcile.log 2>&1\n\n"
    
    # Only install cron entries for helper scripts that actually exist
    if [[ -f "$OPS_DIR/update-metrics.sh" ]]; then
        cron_content+="# Daily metrics update at 4:30 AM (after reconciliation)\n"
        cron_content+="30 4 * * * $CHIMERA_USER $CHIMERA_HOME/ops/update-metrics.sh >> /var/log/chimera/metrics-update.log 2>&1\n\n"
    else
        log_warn "update-metrics.sh not found - skipping its cron job"
    fi
    
    cron_content+="# Daily ML model validation - 3:00 AM (automatic)\n"
    cron_content+="0 3 * * * $CHIMERA_USER cd $CHIMERA_HOME/scout && python3 -m scout.scripts.run_validation --db-path $CHIMERA_HOME/data/chimera.db --time-window 7d >> /var/log/chimera/validation.log 2>&1\n\n"
    
    # Scout runs: locks live in a chimera-owned directory (not world-writable /tmp),
    # and lock contention (flock exit 1) is reported separately from other failures.
    cron_content+="# Weekly Scout run (update wallet roster) - Sundays at 2:00 AM\n"
    cron_content+="0 2 * * 0 $CHIMERA_USER bash -c 'cd \"$CHIMERA_HOME/scout\" || exit 3; flock -n \"$CHIMERA_HOME/data/scout_weekly.lock\" -c \"python3 main.py --output $CHIMERA_HOME/data/roster_new.db\" >> /var/log/chimera/scout.log 2>&1; rc=\$?; if [ \"\$rc\" -eq 1 ]; then echo \"Scout weekly run skipped (already running)\" >> /var/log/chimera/scout.log; exit 0; fi; exit \"\$rc\"'\n\n"
    
    cron_content+="# Scout run twice daily (every 12 hours) - 12:00 AM and 12:00 PM UTC\n"
    cron_content+="0 */12 * * * $CHIMERA_USER bash -c 'cd \"$CHIMERA_HOME/scout\" || exit 3; flock -n \"$CHIMERA_HOME/data/scout_daily.lock\" -c \"python3 main.py --output $CHIMERA_HOME/data/roster_new.db\" >> /var/log/chimera/scout.log 2>&1; rc=\$?; if [ \"\$rc\" -eq 1 ]; then echo \"Scout daily run skipped (already running)\" >> /var/log/chimera/scout.log; exit 0; fi; exit \"\$rc\"'\n\n"
    
    # PostgreSQL-only maintenance jobs: only installed when the postgres
    # container is actually detected at install time.
    if command -v docker > /dev/null 2>&1 && docker ps -aq --filter "name=chimera-postgres" | grep -q .; then
        cron_content+="# Prune old Jito tip history (keep 7 days) - daily at 3:30 AM\n"
        cron_content+="30 3 * * * root docker exec chimera-postgres psql -U chimera -d chimera -c \"DELETE FROM jito_tip_history WHERE created_at < NOW() - INTERVAL '7 days';\" >> /var/log/chimera/db-maintenance.log 2>&1\n\n"
        
        cron_content+="# Prune old dead letter queue entries (keep 30 days) - daily at 3:35 AM\n"
        cron_content+="35 3 * * * root docker exec chimera-postgres psql -U chimera -d chimera -c \"DELETE FROM dead_letter_queue WHERE received_at < NOW() - INTERVAL '30 days';\" >> /var/log/chimera/db-maintenance.log 2>&1\n\n"
    else
        log_warn "chimera-postgres container not detected - skipping PostgreSQL prune jobs"
    fi
    
    if [[ -f "$OPS_DIR/rotate-secrets.sh" ]]; then
        cron_content+="# Secret rotation check (webhook: every 30 days, RPC: every 90 days) - daily at 5:00 AM\n"
        cron_content+="0 5 * * * $CHIMERA_USER $CHIMERA_HOME/ops/rotate-secrets.sh >> /var/log/chimera/secret-rotation.log 2>&1\n\n"
    else
        log_warn "rotate-secrets.sh not found - skipping its cron job"
    fi
    
    if [[ -f "$OPS_DIR/generate-reports.sh" ]]; then
        cron_content+="# Daily PnL summary report - daily at 8:00 PM UTC (20:00)\n"
        cron_content+="0 20 * * * $CHIMERA_USER $CHIMERA_HOME/ops/generate-reports.sh --format=csv --period=1d --type=pnl >> /var/log/chimera/reports.log 2>&1\n\n"
        
        cron_content+="# Weekly full compliance report - Sundays at 6:00 AM UTC\n"
        cron_content+="0 6 * * 0 $CHIMERA_USER $CHIMERA_HOME/ops/generate-reports.sh --format=csv --period=7d --type=full >> /var/log/chimera/reports.log 2>&1\n\n"
        
        cron_content+="# Monthly compliance package - 1st of month at 7:00 AM UTC\n"
        cron_content+="0 7 1 * * $CHIMERA_USER $CHIMERA_HOME/ops/generate-reports.sh --format=csv --period=30d --type=full --package >> /var/log/chimera/reports.log 2>&1\n"
    else
        log_warn "generate-reports.sh not found - skipping its cron jobs"
    fi
    
    printf '%b' "$cron_content" > "$cron_file"
    
    chmod 644 "$cron_file"
    log_info "Cron jobs installed at $cron_file"
}

# Install log rotation
install_logrotate() {
    log_info "Installing log rotation configuration"
    
    cp "$OPS_DIR/logrotate.conf" /etc/logrotate.d/chimera
    chmod 644 /etc/logrotate.d/chimera
    
    # Test configuration
    if logrotate -d /etc/logrotate.d/chimera > /dev/null 2>&1; then
        log_info "Log rotation configuration is valid"
    else
        log_warn "Log rotation configuration may have issues (check manually)"
    fi
}

# Create environment file template
create_env_template() {
    local env_file="$CHIMERA_HOME/config/.env.example"
    
    if [[ ! -f "$env_file" ]]; then
        log_info "Creating environment file template"
        
        cat > "$env_file" << 'EOF'
# Chimera Operator Environment Configuration
# Copy to .env and fill in values

# RPC Configuration
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY
CHIMERA_RPC__FALLBACK_URL=https://your-quicknode-endpoint

# Security
CHIMERA_SECURITY__WEBHOOK_SECRET=your-webhook-secret-here

# Telegram Notifications (optional)
TELEGRAM_BOT_TOKEN=
TELEGRAM_CHAT_ID=

# JWT Secret for dashboard auth
JWT_SECRET=generate-a-strong-secret-here

# Development mode (skip some validations)
CHIMERA_DEV_MODE=false
EOF
        
        chown "$CHIMERA_USER:$CHIMERA_GROUP" "$env_file"
        chmod 640 "$env_file"
    fi
}

# Uninstall everything
uninstall() {
    log_warn "Uninstalling Chimera operations..."
    
    # Stop and disable service
    systemctl stop chimera.service 2>/dev/null || true
    systemctl disable chimera.service 2>/dev/null || true
    rm -f /etc/systemd/system/chimera.service
    systemctl daemon-reload
    
    # Remove cron jobs
    rm -f /etc/cron.d/chimera
    
    # Remove logrotate config
    rm -f /etc/logrotate.d/chimera
    
    log_info "Chimera operations uninstalled"
    log_warn "Note: User, directories, and data were NOT removed. Remove manually if needed:"
    log_warn "  - User: userdel $CHIMERA_USER"
    log_warn "  - Data: rm -rf $CHIMERA_HOME"
    log_warn "  - Logs: rm -rf /var/log/chimera"
}

# Main installation
install() {
    log_info "Installing Chimera operations..."

    # Preflight: all unconditionally-copied files must exist before we mutate anything
    local required_files=(
        "$OPS_DIR/chimera.service"
        "$OPS_DIR/logrotate.conf"
        "$OPS_DIR/backup.sh"
        "$OPS_DIR/reconcile.sh"
    )
    for f in "${required_files[@]}"; do
        if [[ ! -f "$f" ]]; then
            log_error "Required file missing: $f"
            exit 1
        fi
    done

    create_user
    create_directories
    install_systemd
    install_crons
    install_logrotate
    create_env_template
    
    echo ""
    log_info "=========================================="
    log_info "Chimera operations installed successfully!"
    log_info "=========================================="
    echo ""
    log_info "Next steps:"
    echo "  1. Copy and configure environment file:"
    echo "     cp $CHIMERA_HOME/config/.env.example $CHIMERA_HOME/config/.env"
    echo "     nano $CHIMERA_HOME/config/.env"
    echo ""
    echo "  2. Build and deploy the operator binary:"
    echo "     cd $CHIMERA_HOME/operator && cargo build --release"
    echo ""
    echo "  3. Initialize the database:"
    echo "     sqlite3 $CHIMERA_HOME/data/chimera.db < $CHIMERA_HOME/database/schema.sql"
    echo ""
    echo "  4. Start the service:"
    echo "     sudo systemctl start chimera"
    echo "     sudo systemctl status chimera"
    echo ""
    echo "  5. Check logs:"
    echo "     journalctl -u chimera -f"
    echo "     tail -f /var/log/chimera/operator.log"
}

# Parse arguments
check_root

case "${1:-install}" in
    --uninstall|-u)
        uninstall
        ;;
    install|--install|-i|"")
        install
        ;;
    *)
        echo "Usage: $0 [--install | --uninstall]"
        exit 1
        ;;
esac
