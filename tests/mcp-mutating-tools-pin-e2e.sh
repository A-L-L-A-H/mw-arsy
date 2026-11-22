#!/usr/bin/env bash
# MCP Mutating Tools Integration Test
# Tests the 10 new mutating MCP tools against a live panel

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

# Test configuration
PANEL_URL="${PANEL_URL:-https://panel.example.com}"
MCP_TOKEN="${MCP_TOKEN:-}"
ADMIN_TOKEN="${ADMIN_TOKEN:-}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

# Test counter
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

# Test 1: Create site via MCP
test_create_site() {
    local payload='{"domain": "mcp-test.example.com", "runtime": "static", "ssl_mode": "none"}'
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${MCP_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"create_site\",\"arguments\":${payload}}}" \
        "${PANEL_URL}/mcp")
    
    echo "$response" | grep -q '"result"' && echo "$response" | grep -q '"site_id"'
}

# Test 2: Update site via MCP
test_update_site() {
    # First create a site
    local create_payload='{"domain": "mcp-update-test.example.com", "runtime": "static", "ssl_mode": "none"}'
    local create_resp=$(curl -s -X POST \
        -H "Authorization: Bearer ${MCP_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"create_site\",\"arguments\":${create_payload}}}" \
        "${PANEL_URL}/mcp")
    
    local site_id=$(echo "$create_resp" | jq -r '.result.site_id')
    
    # Now update it
    local update_payload="{\"site_id\": \"${site_id}\", \"memory_mb\": 256}"
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${MCP_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"update_site\",\"arguments\":${update_payload}}}" \
        "${PANEL_URL}/mcp")
    
    echo "$response" | grep -q '"result"'
}

# Test 3: Create Docker app via MCP
test_create_app() {
    local payload='{"name": "mcp-test-app", "image": "nginx:alpine", "container_port": 80, "ssl_mode": "none"}'
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${MCP_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"create_app\",\"arguments\":${payload}}}" \
        "${PANEL_URL}/mcp")
    
    echo "$response" | grep -q '"result"' && echo "$response" | grep -q '"container_id"'
}

# Test 4: Deploy Git via MCP
test_deploy_git() {
    # First create a git deploy config
    local create_payload='{"name": "mcp-git-test", "repo_url": "https://github.com/user/static-site.git", "branch": "main", "domain": "mcp-git-test.example.com"}'
    local create_resp=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$create_payload" \
        "${PANEL_URL}/api/git-deploys")
    
    local deploy_id=$(echo "$create_resp" | jq -r '.id')
    
    # Now trigger deploy via MCP
    local payload="{\"deploy_id\": \"${deploy_id}\"}"
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${MCP_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"deploy_git\",\"arguments\":${payload}}}" \
        "${PANEL_URL}/mcp")
    
    echo "$response" | grep -q '"result"'
}

# Test 5: Create database via MCP
test_create_database() {
    local payload='{"name": "mcp_test_db", "engine": "postgresql", "site_id": "00000000-0000-0000-0000-000000000000"}'
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${MCP_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"create_database\",\"arguments\":${payload}}}" \
        "${PANEL_URL}/mcp")
    
    echo "$response" | grep -q '"result"' && echo "$response" | grep -q '"database_id"'
}

# Main
main() {
    log_info "Starting MCP Mutating Tools Integration Tests"
    log_info "Panel URL: ${PANEL_URL}"
    
    if [[ -z "${MCP_TOKEN}" || -z "${ADMIN_TOKEN}" ]]; then
        log_error "MCP_TOKEN and ADMIN_TOKEN must be set"
        exit 1
    fi
    
    run_test "MCP create_site" test_create_site
    run_test "MCP update_site" test_update_site
    run_test "MCP create_app" test_create_app
    run_test "MCP deploy_git" test_deploy_git
    run_test "MCP create_database" test_create_database
    
    echo ""
    log_info "Test Summary: ${TESTS_PASSED}/${TESTS_RUN} passed, ${TESTS_FAILED} failed"
    
    if [[ ${TESTS_FAILED} -gt 0 ]]; then
        exit 1
    fi
}

main "$@"