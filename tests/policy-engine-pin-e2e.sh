#!/usr/bin/env bash
# Policy-as-Code Admission Controller Integration Test

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

# Test 1: Evaluate a policy input that should be denied (privileged container)
test_policy_deny_privileged() {
    local payload='{"input":{"resource":{"kind":"app","name":"test-app","labels":{},"spec":{"privileged":true}},"action":"create","user":{"id":"00000000-0000-0000-0000-000000000000","email":"admin@example.com","role":"admin","groups":[]},"context":{}}}'
    
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/policy/evaluate")
    
    echo "$response" | grep -q '"decision":"deny"' && echo "$response" | grep -q '"matched_policies"'
}

# Test 2: Evaluate a policy input that should be allowed (non-privileged)
test_policy_allow_non_privileged() {
    local payload='{"input":{"resource":{"kind":"app","name":"test-app","labels":{},"spec":{"privileged":false}},"action":"create","user":{"id":"00000000-0000-0000-0000-000000000000","email":"admin@example.com","role":"admin","groups":[]},"context":{}}}'
    
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/policy/evaluate")
    
    echo "$response" | grep -q '"decision":"allow"'
}

# Test 3: Test evaluation with host network (should be denied)
test_policy_deny_host_network() {
    local payload='{"input":{"resource":{"kind":"app","name":"test-app","labels":{},"spec":{"network_mode":"host"}},"action":"create","user":{"id":"00000000-0000-0000-0000-000000000000","email":"admin@example.com","role":"admin","groups":[]},"context":{}}}'
    
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/policy/evaluate")
    
    echo "$response" | grep -q '"decision":"deny"'
}

# Test 4: Create a custom policy
test_policy_create() {
    local payload='{"id":"custom.test-policy","name":"Test Policy","description":"Custom test policy","category":"Custom","severity":"Medium","rego":"package test\ndefault allow = false\nallow { input.resource.kind == \"app\" }","enabled":true}'
    
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/policy/policies")
    
    echo "$response" | grep -q '"id":"custom.test-policy"'
}

# Test 5: List all policies
test_policy_list() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/policy/policies")
    
    echo "$response" | grep -q '\[' && echo "$response" | grep -q '"id"'
}

# Test 6: Get specific policy
test_policy_get() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/policy/policies/security.no-privileged-containers")
    
    echo "$response" | grep -q '"id":"security.no-privileged-containers"'
}

# Test 7: Update a custom policy
test_policy_update() {
    local payload='{"id":"custom.test-policy","name":"Updated Test Policy","description":"Updated test policy","category":"Custom","severity":"High","rego":"package test\ndefault allow = false\nallow { input.resource.kind == \"app\" }\nallow { input.resource.kind == \"database\" }","enabled":true}'
    
    local response=$(curl -s -X PUT \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/policy/policies/custom.test-policy")
    
    echo "$response" | grep -q '"severity":"High"'
}

# Test 8: Delete custom policy
test_policy_delete() {
    local response=$(curl -s -X DELETE \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/policy/policies/custom.test-policy")
    
    echo "$response" | grep -q '"ok":true'
}

# Test 9: Test evaluate with custom policy
test_policy_custom_evaluate() {
    # First create the custom policy
    local create_payload='{"id":"custom.eval-test","name":"Eval Test","description":"Test eval","category":"Custom","severity":"Medium","rego":"package test\ndefault allow = false\nallow { input.resource.labels.allow == \"true\" }","enabled":true}'
    curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d '{"id":"custom.eval-test","name":"Eval Test","description":"Test eval","category":"Custom","severity":"Medium","rego":"package test\ndefault allow = false\nallow { input.resource.labels.allow == \"true\" }","enabled":true}' \
        "${PANEL_URL}/api/policy/policies" > /dev/null
    
    # Now test evaluation
    local payload='{"input":{"resource":{"kind":"app","name":"test","labels":{"allow":"true"},"spec":{}},"action":"create","user":{"id":"00000000-0000-0000-0000-000000000000","email":"admin@example.com","role":"admin","groups":[]},"context":{}}}'
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/policy/test-evaluate")
    
    echo "$response" | grep -q '"decision":"allow"'
}

# Test 10: Test evaluate without allow label (should deny)
test_policy_custom_deny() {
    local payload='{"input":{"resource":{"kind":"app","name":"test","labels":{},"spec":{}},"action":"create","user":{"id":"00000000-0000-0000-0000-000000000000","email":"admin@example.com","role":"admin","groups":[]},"context":{}}}'
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$payload" \
        "${PANEL_URL}/api/policy/test-evaluate")
    
    echo "$response" | grep -q '"decision":"deny"'
}

main() {
    log_info "Starting Policy Engine Integration Tests"
    log_info "Panel URL: ${PANEL_URL}"
    
    if [[ -z "${ADMIN_TOKEN}" ]]; then
        log_error "ADMIN_TOKEN must be set"
        exit 1
    fi
    
    run_test "Policy deny privileged" test_policy_deny_privileged
    run_test "Policy allow non-privileged" test_policy_allow_non_privileged
    run_test "Policy deny host network" test_policy_deny_host_network
    run_test "Policy create custom" test_policy_create
    run_test "Policy list" test_policy_list
    run_test "Policy get" test_policy_get
    run_test "Policy update" test_policy_update
    run_test "Policy delete" test_policy_delete
    run_test "Policy custom eval allow" test_policy_custom_evaluate
    run_test "Policy custom eval deny" test_policy_custom_deny
    
    echo ""
    log_info "Test Summary: ${TESTS_PASSED}/${TESTS_RUN} passed, ${TESTS_FAILED} failed"
    
    if [[ ${TESTS_FAILED} -gt 0 ]]; then
        exit 1
    fi
}

main "$@"