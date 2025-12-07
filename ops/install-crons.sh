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
    
    # Copy scripts to /opt/chimera/ops
    mkdir -p "$CHIMERA_HOME/ops"
    cp "$OPS_DIR/backup.sh" "$CHIMERA_HOME/ops/"
    cp "$OPS_DIR/reconcile.sh" "$CHIMERA_HOME/ops/"
    chown -R "$CHIMERA_USER:$CHIMERA_GROUP" "$CHIMERA_HOME/ops"
    
    # Create crontab entries
    local cron_file="/etc/cron.d/chimera"
    
    cat > "$cron_file" << EOF
# Chimera scheduled tasks
# Managed by install-crons.sh - do not edit manually

SHELL=/bin/bash
PATH=/usr/local/bin:/usr/bin:/bin
CHIMERA_HOME=$CHIMERA_HOME
MAILTO=""

# Daily backup at 3:00 AM
0 3 * * * $CHIMERA_USER $CHIMERA_HOME/ops/backup.sh >> /var/log/chimera/backup.log 2>&1

# Daily reconciliation at 4:00 AM
0 4 * * * $CHIMERA_USER $CHIMERA_HOME/ops/reconcile.sh >> /var/log/chimera/reconcile.log 2>&1

# Weekly Scout run (update wallet roster) - Sundays at 2:00 AM
0 2 * * 0 $CHIMERA_USER cd $CHIMERA_HOME/scout && python3 main.py --output $CHIMERA_HOME/data/roster_new.db >> /var/log/chimera/scout.log 2>&1

# Prune old Jito tip history (keep 7 days) - daily at 3:30 AM
30 3 * * * $CHIMERA_USER sqlite3 $CHIMERA_HOME/data/chimera.db "DELETE FROM jito_tip_history WHERE created_at < datetime('now', '-7 days');" 2>/dev/null

# Prune old dead letter queue entries (keep 30 days) - daily at 3:35 AM
35 3 * * * $CHIMERA_USER sqlite3 $CHIMERA_HOME/data/chimera.db "DELETE FROM dead_letter_queue WHERE received_at < datetime('now', '-30 days');" 2>/dev/null
EOF
    
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
