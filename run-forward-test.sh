#!/bin/bash
# Live Forward Test Orchestration Script
# Runs 21-day forward test with tracer trades, control arms, and verdict evaluation

set -e

# Configuration
PROJECT_ROOT="/Users/mohammad/Documents/GitHub/chimera"
OPERATOR_DB="${PROJECT_ROOT}/operator/data/chimera.db"
VERDICT_SCRIPT="${PROJECT_ROOT}/scout/scripts/verdict.py"
CONFIG_FILE="${PROJECT_ROOT}/config.yaml"

# Experiment configuration
EXPERIMENT_DAYS=21
MIN_TRADES=50
TRACER_CAP=60
MIN_POSITION_SOL=0.02

# Parse arguments
VERBOSE=0
DRY_RUN=0
FORCE=0

while [[ $# -gt 0 ]]; do
    case $1 in
        --verbose|-v)
            VERBOSE=1
            shift
            ;;
        --dry-run|-n)
            DRY_RUN=1
            shift
            ;;
        --force|-f)
            FORCE=1
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [options]"
            echo "Options:"
            echo "  --verbose, -v      Enable verbose logging"
            echo "  --dry-run, -n      Show what would be done without executing"
            echo "  --force, -f        Force start even if experiment already running"
            echo "  --help, -h         Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Logging function
log() {
    local level=$1
    shift
    local message="$@"
    local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[${timestamp}] [${level}] ${message}"
}

log_info() {
    log "INFO" "$@"
}

log_error() {
    log "ERROR" "$@"
}

log_success() {
    log "SUCCESS" "$@"
}

# Check prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."
    
    # Check if operator exists
    if [[ ! -f "${PROJECT_ROOT}/operator/target/debug/chimera" ]]; then
        log_error "Operator binary not found. Build with: make build-operator-debug"
        exit 1
    fi
    
    # Check if verdict script exists
    if [[ ! -f "${VERDICT_SCRIPT}" ]]; then
        log_error "Verdict script not found at ${VERDICT_SCRIPT}"
        exit 1
    fi
    
    # Check if database exists
    if [[ ! -f "${OPERATOR_DB}" ]]; then
        log_error "Operator database not found at ${OPERATOR_DB}"
        exit 1
    fi
    
    # Check if config file exists
    if [[ ! -f "${CONFIG_FILE}" ]]; then
        log_error "Config file not found at ${CONFIG_FILE}"
        exit 1
    fi
    
    log_success "All prerequisites satisfied"
}

# Check if experiment is already running
check_experiment_status() {
    log_info "Checking experiment status..."
    
    local status=$(sqlite3 "${OPERATOR_DB}" "SELECT status FROM experiment_manifest WHERE status = 'running' LIMIT 1;")
    
    if [[ -n "${status}" ]] && [[ "${status}" == "running" ]] && [[ ${FORCE} -eq 0 ]]; then
        log_error "Experiment is already running. Use --force to start anyway."
        log_info "Current experiment status: ${status}"
        exit 1
    fi
    
    log_info "No running experiment found. Ready to start."
}

# Create new experiment run record
create_experiment_run() {
    log_info "Creating new experiment run record..."
    
    local run_id="ft-$(date +%Y%m%d-%H%M%S)"
    local start_time=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    local settings=$(cat "${CONFIG_FILE}")
    
    # Insert experiment manifest
    sqlite3 "${OPERATOR_DB}" << EOF
INSERT INTO experiment_manifest (
    run_id,
    t0,
    settings,
    status,
    start_time,
    total_trades,
    tracer_trades,
    toxic_wallets,
    total_wallets
) VALUES (
    '${run_id}',
    '${start_time}',
    '${settings}',
    'running',
    '${start_time}',
    0,
    0,
    0,
    0
);
EOF
    
    log_success "Experiment run created: ${run_id}"
    echo "${run_id}"
}

# Wait for experiment to complete
wait_for_completion() {
    local run_id=$1
    
    log_info "Waiting for experiment to complete (${EXPERIMENT_DAYS} days, ${MIN_TRADES} minimum trades)..."
    
    local elapsed_days=0
    local total_trades=0
    
    while true; do
        sleep 60  # Check every minute
        
        # Get experiment status
        local status=$(sqlite3 "${OPERATOR_DB}" "SELECT status FROM experiment_manifest WHERE run_id = '${run_id}';")
        
        # Get current statistics
        local stats=$(sqlite3 "${OPERATOR_DB}" << EOF
SELECT 
    total_trades,
    tracer_trades,
    toxic_wallets,
    total_wallets,
    julianday('now') - julianday(start_time) as days_elapsed
FROM experiment_manifest 
WHERE run_id = '${run_id}';
EOF
)
        
        if [[ -n "${stats}" ]]; then
            read -r total_trades tracer_trades toxic_wallets total_wallets days_elapsed <<< "${stats}"
            elapsed_days=${days_elapsed%.*}
        fi
        
        # Check if experiment completed naturally
        if [[ "${status}" == "completed" ]] || [[ "${status}" == "aborted" ]]; then
            log_info "Experiment ended with status: ${status}"
            break
        fi
        
        # Check if experiment should complete
        if [[ ${elapsed_days} -ge ${EXPERIMENT_DAYS} ]] && [[ ${total_trades} -ge ${MIN_TRADES} ]]; then
            log_info "Experiment completed: ${elapsed_days} days elapsed, ${total_trades} trades executed"
            break
        fi
        
        # Check for abort conditions
        check_abort_conditions "${run_id}"
        
        # Progress update (every hour)
        if [[ ${VERBOSE} -eq 1 ]] && [[ $(date +%M) == "00" ]]; then
            log_info "Progress: Day ${elapsed_days}/${EXPERIMENT_DAYS}, ${total_trades}/${MIN_TRADES} trades, ${tracer_trades} tracer executions"
        fi
    done
    
    log_success "Experiment completed"
}

# Check for abort conditions
check_abort_conditions() {
    local run_id=$1
    
    # Check credit exhaustion
    local credit_status=$(check_credits)
    
    if [[ "${credit_status}" == "exhausted" ]]; then
        log_error "Credit budget exhausted - aborting experiment"
        abort_experiment "${run_id}" "Credit budget exhausted"
        exit 1
    fi
    
    # Check tracer cap
    local tracer_count=$(sqlite3 "${OPERATOR_DB}" "SELECT tracer_trades FROM experiment_manifest WHERE run_id = '${run_id}';")
    
    if [[ ${tracer_count} -ge ${TRACER_CAP} ]]; then
        log_info "Tracer cap reached (${TRACER_CAP} trades) - tapering sample rate"
        # Sample rate tapering is handled in operator logic
    fi
}

# Check credit status
check_credits() {
    local helius_usage_file="${PROJECT_ROOT}/scout/helius_credit_tracker.py"
    
    if [[ -f "${helius_usage_file}" ]]; then
        local credits_remaining=$(cd "${PROJECT_ROOT}/scout" && python helius_credit_tracker.py 2>/dev/null | grep "Credits remaining" | awk '{print $3}' || echo "0")
        
        if [[ "${credits_remaining}" -lt 10000 ]]; then
            echo "exhausted"
            return
        fi
    fi
    
    echo "ok"
}

# Abort experiment
abort_experiment() {
    local run_id=$1
    local reason=$2
    
    log_info "Aborting experiment: ${reason}"
    
    sqlite3 "${OPERATOR_DB}" << EOF
UPDATE experiment_manifest 
SET 
    status = 'aborted',
    end_time = datetime('now'),
    verdict = 'KILL',
    verdict_time = datetime('now'),
    verdict_reasons = '["${reason}"]'
WHERE run_id = '${run_id}';
EOF
}

# Evaluate experiment verdict
evaluate_verdict() {
    local run_id=$1
    
    log_info "Evaluating experiment verdict..."
    
    local verdict_output=$(python "${VERDICT_SCRIPT}" \
        --db-path "${OPERATOR_DB}" \
        --output "${PROJECT_ROOT}/verdict-$(date +%Y%m%d-%H%M%S).json" \
        2>&1)
    
    local verdict_exit_code=$?
    
    echo "${verdict_output}"
    
    if [[ ${verdict_exit_code} -eq 0 ]]; then
        local verdict=$(echo "${verdict_output}" | python -c "import sys, json; print(json.load(sys.stdin)['verdict'])" 2>/dev/null || echo "UNKNOWN")
        log_success "Verdict: ${verdict}"
        
        # Update experiment manifest with verdict
        sqlite3 "${OPERATOR_DB}" << EOF
UPDATE experiment_manifest 
SET 
    status = 'completed',
    end_time = datetime('now'),
    verdict = '${verdict}',
    verdict_time = datetime('now'),
    verdict_reasons = '${verdict_output}'
WHERE run_id = '${run_id}';
EOF
        
        return 0
    else
        log_error "Verdict evaluation failed with exit code ${verdict_exit_code}"
        return 1
    fi
}

# Main execution
main() {
    log_info "Starting live forward test orchestration"
    
    check_prerequisites
    check_experiment_status
    
    if [[ ${DRY_RUN} -eq 1 ]]; then
        log_info "Dry run mode - no changes will be made"
        exit 0
    fi
    
    # Create new experiment run
    local run_id=$(create_experiment_run)
    
    log_info "Experiment run ID: ${run_id}"
    log_info "Configuration: ${EXPERIMENT_DAYS} days, ${MIN_TRADES} minimum trades, ${TRACER_CAP} tracer cap"
    
    # Start operator in background
    log_info "Starting operator..."
    
    cd "${PROJECT_ROOT}/operator"
    
    # Run operator with experiment mode
    RUST_LOG=info RUST_BACKTRACE=1 ./target/debug/chimera \
        --config "${CONFIG_FILE}" \
        --mode paper \
        --experiment-enabled &
    
    local operator_pid=$!
    echo "${operator_pid}" > "${PROJECT_ROOT}/operator.pid"
    
    log_info "Operator started with PID: ${operator_pid}"
    
    # Wait for experiment to complete
    wait_for_completion "${run_id}"
    
    # Stop operator
    log_info "Stopping operator..."
    kill "${operator_pid}" 2>/dev/null || true
    rm -f "${PROJECT_ROOT}/operator.pid"
    
    # Evaluate verdict
    evaluate_verdict "${run_id}"
    
    log_success "Forward test completed"
}

# Run main function
main
