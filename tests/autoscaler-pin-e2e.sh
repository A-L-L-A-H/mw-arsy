#!/usr/bin/env bash
# Horizontal Autoscaler Integration Test

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

PANEL_URL="${PANEL_URL:-https://panel.example.com}"
ADMIN_TOKEN="${ADMIN_TOKEN:-}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

run_test() {
    local test_name="$1"
    local test_fn="$2"
    TESTS_RUN=$((TESTS_RUN + 1))
    log_info "Running test: ${test_name}"
    if $test_fn; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_info "PASSED: ${test_name}"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_error "FAILED: ${test_name}"
    fi
}

# Test 1: Create autoscaling rule for an app
test_autoscaler_create_rule() {
    local payload='{"resource_type": "app", "resource_id": "00000000-0000-0000-0000-000000000000", "name": "test-scale", "min_replicas": 1, "max_replicas": 5, "cooldown_seconds": 300, "triggers": [{"type": "cpu", "threshold_percent": 80}]}'
    
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/autoscaler/rules")
    
    echo "$response" | grep -q '"id"' && echo "$response" | grep -q '"name":"test-scale"'
}

# Test 2: List autoscaling rules
test_autoscaler_list_rules() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/autoscaler/rules")
    
    echo "$response" | grep -q '\[' && echo "$response" | grep -q '"id"'
}

# Test 3: Get autoscaler status
test_autoscaler_status() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/autoscaler/status")
    
    echo "$response" | grep -q '\[' && echo "$response" | grep -q '"rule_id"'
}

# Test 4: Get autoscaler history
test_autoscaler_history() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/autoscaler/history?limit=10")
    
    echo "$response" | grep -q '\[' && echo "$response" | grep -q '"action"'
}

# Test 5: Update autoscaling rule
test_autoscaler_update_rule() {
    # First create a rule
    local payload='{"resource_type": "app", "resource_id": "00000000-0000-0000-0000-000000000000", "name": "update-test", "min_replicas": 1, "max_replicas": 3, "cooldown_seconds": 300, "triggers": [{"type": "cpu", "threshold_percent": 70}]}'
    local create_resp=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/autoscaler/rules")
    
    local rule_id=$(echo "$create_resp" | jq -r '.id')
    
    # Update the rule
    local update_payload='{"max_replicas": 10}'
    local response=$(curl -s -X PUT \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$update_payload" \
        "${PANEL_URL}/api/autoscaler/rules/${rule_id}")
    
    echo "$response" | grep -q '"max_replicas":10'
}

# Test 6: Delete autoscaling rule
test_autoscaler_delete_rule() {
    # Create a rule to delete
    local payload='{"resource_type": "app", "resource_id": "00000000-0000-0000-0000-000000000000", "name": "delete-test", "min_replicas": 1, "max_replicas": 3, "cooldown_seconds": 300, "triggers": [{"type": "cpu", "threshold_percent": 80}]}'
    local create_resp=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/autoscaler/rules")
    
    local rule_id=$(echo "$create_resp" | jq -r '.id')
    
    # Delete the rule
    local response=$(curl -s -X DELETE \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/autoscaler/rules/${rule_id}")
    
    echo "$response" | grep -q '"ok":true'
}

main() {
    log_info "Starting Horizontal Autoscaler Integration Tests"
    log_info "Panel URL: ${PANEL_URL}"
    
    if [[ -z "${ADMIN_TOKEN}" ]]; then
        log_error "ADMIN_TOKEN must be set"
        exit 1
    fi
    
    run_test "Autoscaler create rule" test_autoscaler_create_rule
    run_test "Autoscaler list rules" test_autoscaler_list_rules
    run_test "Autoscaler status" test_autoscaler_status
    run_test "Autoscaler history" test_autoscaler_history
    run_test "Autoscaler update rule" test_autoscaler_update_rule
    run_test "Autoscaler delete rule" test_autoscaler_delete_rule
    
    echo ""
    log_info "Test Summary: ${TESTS_PASSED}/${TESTS_RUN} passed, ${TESTS_FAILED} failed"
    
    if [[ ${TESTS_FAILED} -gt 0 ]]; then
        exit 1
    fi
}

main "$@"