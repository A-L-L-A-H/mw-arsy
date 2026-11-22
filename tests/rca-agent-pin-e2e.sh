#!/usr/bin/env bash
# RCA Agent Integration Test

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

# Test 1: Manual diagnosis
test_rca_manual_diagnosis() {
    local payload='{"trigger":{"type":"Manual","description":"Test diagnosis"},"context":"Test context","target_server_id":null,"time_range_hours":24}'
    
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/rca/diagnose")
    
    echo "$response" | grep -q '"diagnosis_id"' && echo "$response" | grep -q '"status":"completed"'
}

# Test 2: Alert-triggered diagnosis
test_rca_alert_triggered() {
    # First create an alert
    local alert_payload='{"type": "disk", "server_id": "00000000-0000-0000-0000-000000000000", "message": "Disk usage critical"}'
    local alert_resp=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$alert_payload" \
        "${PANEL_URL}/api/alerts")
    
    local alert_id=$(echo "$alert_resp" | jq -r '.id')
    
    # Now trigger diagnosis
    local payload="{\"trigger\":{\"type\":\"AlertFired\",\"alert_id\":\"${alert_id}\",\"alert_type\":\"disk\"},\"target_server_id\":null,\"time_range_hours\":24}"
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/rca/diagnose")
    
    echo "$response" | grep -q '"diagnosis_id"' && echo "$response" | grep -q '"root_cause"'
}

# Test 3: Deploy failure diagnosis
test_rca_deploy_failed() {
    # Create a failed deploy
    local deploy_payload='{"name": "rca-test-deploy", "repo_url": "https://github.com/user/broken-app.git", "branch": "main", "domain": "rca-test.example.com"}'
    local deploy_resp=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$deploy_payload" \
        "${PANEL_URL}/api/git-deploys")
    
    local deploy_id=$(echo "$deploy_resp" | jq -r '.id')
    
    # Trigger deploy to make it fail
    curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d '{}' \
        "${PANEL_URL}/api/git-deploys/${deploy_id}/deploy" > /dev/null
    
    # Wait a bit for deploy to fail
    sleep 5
    
    # Trigger diagnosis
    local payload="{\"trigger\":{\"type\":\"DeployFailed\",\"deploy_id\":\"${deploy_id}\",\"commit_hash\":\"abc123\"},\"target_server_id\":null,\"time_range_hours\":24}"
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/rca/diagnose")
    
    echo "$response" | grep -q '"diagnosis_id"' && echo "$response" | grep -q '"recommended_actions"'
}

# Test 2: Get diagnosis history
test_rca_history() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/rca/diagnoses?limit=10&offset=0")
    
    echo "$response" | grep -q '\[' && echo "$response" | grep -q '"diagnosis_id"'
}

# Test 2: Get current diagnosis
test_rca_current() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/rca/current")
    
    # May be null if no diagnosis in progress
    echo "$response" | grep -q 'null' || echo "$response" | grep -q '"diagnosis_id"'
}

# Test 5: Submit feedback
test_rca_feedback() {
    # First get a diagnosis ID
    local history=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/rca/diagnoses?limit=1&offset=0")
    
    local diagnosis_id=$(echo "$history" | jq -r '.[0].id')
    
    if [[ -z "${diagnosis_id}" || "${diagnosis_id}" == "null" ]]; then
        log_warn "No diagnosis history available, skipping feedback test"
        return 0
    fi
    
    local payload='{"rating": 4, "accurate": true, "comments": "Good diagnosis"}'
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/rca/diagnoses/${diagnosis_id}/feedback")
    
    echo "$response" | grep -q '"ok":true'
}

# Test 3: Get RCA config
test_rca_config() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/rca/config")
    
    echo "$response" | grep -q '"enabled"' && echo "$response" | grep -q '"llm_provider"'
}

# Test 4: Update RCA config (admin only)
test_rca_config_update() {
    local payload='{"rca_enabled": true, "rca_auto_trigger_alert": true, "rca_auto_trigger_deploy": true, "rca_llm_provider": "anthropic", "rca_llm_model": "claude-3-5-sonnet-20241022"}'
    local response=$(curl -s -X PUT \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/rca/config")
    
    echo "$response" | grep -q '"ok":true'
}

main() {
    log_info "Starting RCA Agent Integration Tests"
    log_info "Panel URL: ${PANEL_URL}"
    
    if [[ -z "${ADMIN_TOKEN}" ]]; then
        log_error "ADMIN_TOKEN must be set"
        exit 1
    fi
    
    run_test "RCA manual diagnosis" test_rca_manual_diagnosis
    run_test "RCA alert triggered" test_rca_alert_triggered
    run_test "RCA deploy failed" test_rca_deploy_failed
    run_test "RCA history" test_rca_history
    run_test "RCA current" test_rca_current
    run_test "RCA feedback" test_rca_feedback
    run_test "RCA config get" test_rca_config
    run_test "RCA config update" test_rca_config_update
    
    echo ""
    log_info "Test Summary: ${TESTS_PASSED}/${TESTS_RUN} passed, ${TESTS_FAILED} failed"
    
    if [[ ${TESTS_FAILED} -gt 0 ]]; then
        exit 1
    fi
}

main "$@"