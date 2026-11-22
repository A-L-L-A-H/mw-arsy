#!/usr/bin/env bash
# GitOps Fleet Sync Controller Integration Test

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

# Test 1: Configure GitOps controller
test_gitops_configure() {
    local payload='{"enabled": true, "git_repo_url": "https://github.com/user/dockpanel-gitops.git", "git_branch": "main", "sync_interval_seconds": 300, "dry_run": true, "auto_reconcile": false, "prune_orphaned": false}'
    
    local response=$(curl -s -X PUT \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/gitops/config")
    
    echo "$response" | grep -q '"ok":true'
}

# Test 2: Preview reconciliation (dry run)
test_gitops_preview() {
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/gitops/preview")
    
    echo "$response" | grep -q '"total_resources"' && echo "$response" | grep -q '"git_commit"'
}

# Test 3: Get GitOps status
test_gitops_status() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/gitops/status")
    
    echo "$response" | grep -q '"enabled"' && echo "$response" | grep -q '"config"'
}

# Test 4: Full reconciliation (dry run mode)
test_gitops_reconcile_dry_run() {
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/gitops/reconcile")
    
    echo "$response" | grep -q '"total_resources"' && echo "$response" | grep -q '"dry_run"'
}

# Test 5: Get last reconciliation report
test_gitops_report() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/gitops/reconcile")
    
    echo "$response" | grep -q '"timestamp"' && echo "$response" | grep -q '"git_commit"'
}

main() {
    log_info "Starting GitOps Controller Integration Tests"
    log_info "Panel URL: ${PANEL_URL}"
    
    if [[ -z "${ADMIN_TOKEN}" ]]; then
        log_error "ADMIN_TOKEN must be set"
        exit 1
    fi
    
    run_test "GitOps configure" test_gitops_configure
    run_test "GitOps preview" test_gitops_preview
    run_test "GitOps status" test_gitops_status
    run_test "GitOps reconcile (dry run)" test_gitops_reconcile_dry_run
    run_test "GitOps report" test_gitops_report
    
    echo ""
    log_info "Test Summary: ${TESTS_PASSED}/${TESTS_RUN} passed, ${TESTS_FAILED} failed"
    
    if [[ ${TESTS_FAILED} -gt 0 ]]; then
        exit 1
    fi
}

main "$@"