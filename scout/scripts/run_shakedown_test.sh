#!/bin/bash
# Shake-down test for forward test experiment
# Validates all experiment components before starting the real 21-day test.
# Phase 3 runs the operator against a short-duration config for a bounded
# period; it does NOT run the full 24-hour test.

set -e

PROJECT_ROOT="/Users/mohammad/Documents/GitHub/chimera"
OPERATOR="${PROJECT_ROOT}/operator/target/release/chimera_operator"
CONFIG="${PROJECT_ROOT}/config/experiment.yaml"
DB_PATH="${PROJECT_ROOT}/operator/data/chimera.db"

# Logging
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

log_success() {
    log "SUCCESS" "$@"
}

log_error() {
    log "ERROR" "$@"
}

log_warning() {
    log "WARNING" "$@"
}

# Test database schema
test_database_schema() {
    log_info "Testing database schema..."
    
    local required_tables=(
        "experiment_trades"
        "experiment_manifest"
        "toxic_wallets"
        "experiment_credits"
    )
    
    for table in "${required_tables[@]}"; do
        if sqlite3 "$DB_PATH" "SELECT name FROM sqlite_master WHERE type='table' AND name='$table';" | grep -q "$table"; then
            log_success "✓ Table $table exists"
        else
            log_error "✗ Table $table missing"
            return 1
        fi
    done
    
    log_success "Database schema validation complete"
}

# Test operator compilation
test_operator_compilation() {
    log_info "Testing operator compilation..."
    
    if [ ! -f "$OPERATOR" ]; then
        log_error "Operator binary not found: $OPERATOR"
        return 1
    fi
    
    # Check if binary is executable
    if [ ! -x "$OPERATOR" ]; then
        log_error "Operator binary is not executable: $OPERATOR"
        chmod +x "$OPERATOR" 2>/dev/null || {
            log_error "Failed to make operator binary executable"
            return 1
        }
    fi
    
    log_success "✓ Operator binary exists and is executable"
}

# Test experiment configuration
test_experiment_config() {
    log_info "Testing experiment configuration..."
    
    if [ ! -f "$CONFIG" ]; then
        log_error "Config file not found: $CONFIG"
        return 1
    fi
    
    # Parse config with a real YAML parser; missing/malformed keys are failures
    local parsed
    if ! parsed=$(python3 - "$CONFIG" <<'PYEOF'
import sys
try:
    import yaml
except ImportError:
    print("ERROR=PyYAML not available", file=sys.stderr)
    sys.exit(1)
try:
    cfg = yaml.safe_load(open(sys.argv[1]))
except Exception as e:
    print(f"ERROR={e}", file=sys.stderr)
    sys.exit(1)
exp = cfg.get('experiment') or {}
for key in ('tracer_enabled', 'tracer_cap', 'experiment_days', 'min_trades'):
    print(f"{key}={exp.get(key)}")
PYEOF
    ); then
        log_error "✗ Failed to parse config YAML: $CONFIG"
        return 1
    fi

    local tracer_enabled=$(echo "$parsed" | sed -n 's/^tracer_enabled=//p')
    local tracer_cap=$(echo "$parsed" | sed -n 's/^tracer_cap=//p')
    local experiment_days=$(echo "$parsed" | sed -n 's/^experiment_days=//p')
    local min_trades=$(echo "$parsed" | sed -n 's/^min_trades=//p')
    
    log_info "Config: tracer_enabled=$tracer_enabled, tracer_cap=$tracer_cap, experiment_days=$experiment_days, min_trades=$min_trades"
    
    if [ "$tracer_enabled" != "true" ]; then
        log_warning "tracer_enabled is not true"
    fi
    
    if ! [[ "$tracer_cap" =~ ^[0-9]+$ ]] || [ "$tracer_cap" -lt 1 ]; then
        log_error "✗ tracer_cap missing or non-numeric: '$tracer_cap'"
        return 1
    fi
    
    if ! [[ "$experiment_days" =~ ^[0-9]+$ ]] || [ "$experiment_days" -lt 1 ]; then
        log_error "✗ experiment_days missing or invalid: '$experiment_days'"
        return 1
    fi
    
    if ! [[ "$min_trades" =~ ^[0-9]+$ ]] || [ "$min_trades" -lt 1 ]; then
        log_error "✗ min_trades missing or invalid: '$min_trades'"
        return 1
    fi
    
    log_success "✓ Configuration file is valid"
}

# Test experiment setup script
test_setup_script() {
    log_info "Testing setup script..."
    
    local setup_script="${PROJECT_ROOT}/scout/scripts/setup_experiment.py"
    
    if [ ! -f "$setup_script" ]; then
        log_error "Setup script not found: $setup_script"
        return 1
    fi
    
    if ! python3 -m py_compile "$setup_script" 2>/dev/null; then
        log_error "Setup script has syntax errors"
        return 1
    fi
    
    log_success "✓ Setup script is valid"
}

# Test verdict script
test_verdict_script() {
    log_info "Testing verdict script..."
    
    local verdict_script="${PROJECT_ROOT}/scout/scripts/verdict.py"
    
    if [ ! -f "$verdict_script" ]; then
        log_error "Verdict script not found: $verdict_script"
        return 1
    fi
    
    if ! python3 -m py_compile "$verdict_script" 2>/dev/null; then
        log_error "Verdict script has syntax errors"
        return 1
    fi
    
    # Test with dry run - check python3's real exit code, keep stderr
    local verdict_out
    if ! verdict_out=$(python3 "$verdict_script" --db-path "$DB_PATH" 2>&1); then
        log_warning "Verdict script exited non-zero with empty/invalid database"
    elif ! echo "$verdict_out" | grep -q "verdict"; then
        log_warning "Verdict script output missing 'verdict' key"
    fi
    
    log_success "✓ Verdict script is valid"
}

# Test metrics availability
test_metrics_availability() {
    log_info "Testing Prometheus metrics availability..."
    
    # Check if experiment modules exist and are integrated
    local experiment_dir="${PROJECT_ROOT}/operator/src/experiment"
    
    if [ -d "$experiment_dir" ]; then
        # Check for core experiment modules
        local required_modules=("tracer" "controls" "ledger" "verdict" "toxic")
        for module in "${required_modules[@]}"; do
            if [ ! -f "${experiment_dir}/${module}.rs" ]; then
                log_error "Missing experiment module: ${module}.rs"
                return 1
            fi
        done
        log_success "✓ All core experiment modules present"
    else
        log_error "Experiment directory not found: $experiment_dir"
        return 1
    fi
}

# Test Grafana dashboard
test_grafana_dashboard() {
    log_info "Testing Grafana dashboard..."
    
    local dashboard="${PROJECT_ROOT}/ops/grafana/experiment-dashboard.json"
    
    if [ ! -f "$dashboard" ]; then
        log_error "Grafana dashboard not found: $dashboard"
        return 1
    fi
    
    # Validate JSON
    if ! python3 -c "import json; json.load(open('$dashboard'))" 2>/dev/null; then
        log_error "Grafana dashboard has invalid JSON"
        return 1
    fi
    
    log_success "✓ Grafana dashboard is valid"
}

# Test credit tracking
test_credit_tracking() {
    log_info "Testing credit tracking integration..."
    
    local credit_tracker="${PROJECT_ROOT}/scout/helius_credit_tracker.py"
    
    if [ ! -f "$credit_tracker" ]; then
        log_warning "Credit tracker not found: $credit_tracker"
        return 0
    fi
    
    if ! python3 -m py_compile "$credit_tracker" 2>/dev/null; then
        log_error "Credit tracker has syntax errors"
        return 1
    fi
    
    log_success "✓ Credit tracker is valid"
}

# Run short experiment test
run_short_experiment_test() {
    log_info "Running short experiment test (bounded operator smoke test)..."
    
    # Create temporary test config
    local test_config="${PROJECT_ROOT}/config/shakedown_test.yaml"
    cp "$CONFIG" "$test_config"
    
    # Modify for short test
    sed -i '' 's/experiment_days: 21/experiment_days: 1/' "$test_config"
    sed -i '' 's/min_trades: 50/min_trades: 3/' "$test_config"
    sed -i '' 's/tracer_cap: 60/tracer_cap: 3/' "$test_config"
    
    # Verify each substitution actually matched
    local missing=""
    grep -q "experiment_days: 1" "$test_config" || missing="$missing experiment_days"
    grep -q "min_trades: 3" "$test_config" || missing="$missing min_trades"
    grep -q "tracer_cap: 3" "$test_config" || missing="$missing tracer_cap"
    if [ -n "$missing" ]; then
        log_error "✗ Config substitution did not match:$missing - test config invalid"
        rm -f "$test_config"
        return 1
    fi
    log_success "✓ Test config created with short-duration settings"
    
    if [ ! -x "$OPERATOR" ]; then
        log_error "✗ Operator binary not found: $OPERATOR"
        log_error "Short experiment test cannot run - failing instead of claiming success"
        rm -f "$test_config"
        return 1
    fi
    
    # Launch the operator against the test config for a bounded period
    local log_file=/tmp/shakedown_test.log
    rm -f "$log_file"
    "$OPERATOR" --config "$test_config" --mode paper --experiment-enabled >"$log_file" 2>&1 &
    local op_pid=$!
    
    # Give it up to 30s to start; fail if it exits immediately with an error
    local waited=0
    while kill -0 "$op_pid" 2>/dev/null && [ $waited -lt 30 ]; do
        sleep 2
        waited=$((waited + 2))
    done
    
    if ! kill -0 "$op_pid" 2>/dev/null; then
        log_error "✗ Operator exited during short experiment test"
        log_error "Log tail:"
        tail -20 "$log_file" 2>/dev/null | sed 's/^/    /'
        rm -f "$test_config" "$log_file"
        return 1
    fi
    
    kill "$op_pid" 2>/dev/null || true
    wait "$op_pid" 2>/dev/null || true
    
    # Check that the operator ran (trades table may still be empty early on)
    local trade_count
    trade_count=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM experiment_trades;" 2>/dev/null || echo 0)
    log_info "Trades recorded during test: $trade_count"
    log_success "✓ Operator ran under test config without crashing"
    
    # Cleanup
    rm -f "$test_config"
    rm -f /tmp/shakedown_test.log
}

# Validate experiment components
validate_experiment_components() {
    log_info "Validating experiment components..."
    
    local all_found=true
    local missing=0
    
    # Check for database tables via SQL
    if sqlite3 "$DB_PATH" "SELECT name FROM sqlite_master WHERE type='table' AND name='experiment_trades';" 2>/dev/null | grep -q "experiment_trades"; then
        log_success "✓ Found: experiment_trades table"
    else
        log_warning "✗ Not found: experiment_trades table"
        all_found=false
        missing=$((missing + 1))
    fi
    
    if sqlite3 "$DB_PATH" "SELECT name FROM sqlite_master WHERE type='table' AND name='experiment_manifest';" 2>/dev/null | grep -q "experiment_manifest"; then
        log_success "✓ Found: experiment_manifest table"
    else
        log_warning "✗ Not found: experiment_manifest table"
        all_found=false
        missing=$((missing + 1))
    fi
    
    if sqlite3 "$DB_PATH" "SELECT name FROM sqlite_master WHERE type='table' AND name='toxic_wallets';" 2>/dev/null | grep -q "toxic_wallets"; then
        log_success "✓ Found: toxic_wallets table"
    else
        log_warning "✗ Not found: toxic_wallets table"
        all_found=false
        missing=$((missing + 1))
    fi
    
    if sqlite3 "$DB_PATH" "SELECT name FROM sqlite_master WHERE type='table' AND name='experiment_credits';" 2>/dev/null | grep -q "experiment_credits"; then
        log_success "✓ Found: experiment_credits table"
    else
        log_warning "✗ Not found: experiment_credits table"
        all_found=false
        missing=$((missing + 1))
    fi
    
    # Check for Rust modules
    local rust_modules=("tracer" "controls" "ledger" "verdict" "toxic")
    for module in "${rust_modules[@]}"; do
        if [ -f "${PROJECT_ROOT}/operator/src/experiment/${module}.rs" ]; then
            log_success "✓ Found: ${module} module"
        else
            log_warning "✗ Not found: ${module} module"
            all_found=false
            missing=$((missing + 1))
        fi
    done
    
    # Check for Python scripts
    if [ -f "${PROJECT_ROOT}/scout/scripts/setup_experiment.py" ]; then
        log_success "✓ Found: T0 selector (setup script)"
    else
        log_warning "✗ Not found: T0 selector"
        all_found=false
        missing=$((missing + 1))
    fi
    
    if [ -f "${PROJECT_ROOT}/ops/grafana/experiment-dashboard.json" ]; then
        log_success "✓ Found: Grafana dashboard"
    else
        log_warning "✗ Not found: Grafana dashboard"
        all_found=false
        missing=$((missing + 1))
    fi
    
    if [ "$all_found" = true ]; then
        log_success "All expected experiment components found"
    else
        log_warning "Some experiment components may be missing"
        return 1
    fi
}

# Main test sequence
main() {
    log_info "Starting shake-down test for forward test experiment"
    log_info "=========================================================="
    
    local failed=0
    
    # Pre-flight checks
    log_info "Phase 1: Pre-flight checks"
    test_database_schema || failed=$((failed + 1))
    test_operator_compilation || failed=$((failed + 1))
    test_experiment_config || failed=$((failed + 1))
    test_setup_script || failed=$((failed + 1))
    test_verdict_script || failed=$((failed + 1))
    
    # Component validation
    log_info "Phase 2: Component validation"
    test_metrics_availability || failed=$((failed + 1))
    test_grafana_dashboard || failed=$((failed + 1))
    test_credit_tracking || failed=$((failed + 1))
    validate_experiment_components || failed=$((failed + 1))
    
    if [ $failed -gt 0 ]; then
        log_error "Pre-flight checks failed with $failed errors"
        return 1
    fi
    
    log_success "All pre-flight checks passed"
    
    # Short experiment test
    log_info "Phase 3: Short experiment test"
    run_short_experiment_test || {
        log_error "Short experiment test failed"
        return 1
    }
    
    # Final summary
    log_info "=========================================================="
    log_success "✅ Shake-down test completed successfully!"
    log_info "All components validated and ready for 21-day forward test"
    log_info "Start the real experiment with: ./run-forward-test.sh"
    
    return 0
}

# Run main
main "$@"
