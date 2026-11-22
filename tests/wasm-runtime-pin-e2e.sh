#!/usr/bin/env bash
# Wasm Runtime Integration Test
# Tests Wasm component upload, execution, and management

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

# Test 1: Upload a simple Wasm component
test_wasm_upload() {
    # Create a minimal valid Wasm component (WASI preview2 command)
    # This is a minimal component that just exits successfully
    local wasm_bytes=$(cat <<'WASM' | xxd -r -p
00 61 73 6d 01 00 00 00 01 07 01 60 00 01 7f 03 02 01 00 07 07 01 03 65 6e
76 00 00 03 65 6e 76 01 00 04 65 6e 76 02 00 04 65 6e 76 03 00 04 65 6e
76 04 00 04 65 6e 76 05 00 04 65 6e 76 06 00 04 65 6e 76 07 00 04 65 6e
76 08 00 04 65 6e 76 09 00 04 65 6e 76 0a 00 04 65 6e 76 0b 00 04 65 6e
76 0c 00 04 65 6e 76 0d 00 04 65 6e 76 0e 00 04 65 6e 76 0f 00 03 02 01
00 0a 07 01 02 02 04 03 01 01 01 00 20 00 41 2a 0b 0b
WASM
)
    
    # Create multipart form data
    local boundary="----WebKitFormBoundary$(date +%s)"
    local form_data="--${boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\ntest-component\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n1.0.0\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nTest Wasm component\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"component\"; filename=\"test.wasm\"\r\nContent-Type: application/wasm\r\n\r\n${wasm_bytes}\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"resource_limits\"\r\n\r\n{\"max_memory_mb\":64,\"max_fuel\":10000000,\"epoch_deadline\":1000,\"max_table_elements\":1024,\"allowed_hosts\":[]}\r\n--${boundary}--\r\n"
    
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: multipart/form-data; boundary=${boundary}" \
        --data-binary "${form_data}" \
        "${PANEL_URL}/api/wasm/upload")
    
    echo "$response" | grep -q '"id"' && echo "$response" | grep -q '"name":"test-component"'
}

# Test 2: List Wasm components
test_wasm_list() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/wasm/components")
    
    echo "$response" | grep -q '\[' && echo "$response" | grep -q '"id"'
}

# Test 3: Execute Wasm component
test_wasm_execute() {
    # First upload a component
    local wasm_bytes=$(cat <<'WASM' | xxd -r -p
00 61 73 6d 01 00 00 00 01 07 01 60 00 01 7f 03 02 01 00 07 07 01 03 65 6e
76 00 00 03 65 6e 76 01 00 04 65 6e 76 02 00 04 65 6e 76 03 00 04 65 6e
76 04 00 04 65 6e 76 05 00 04 65 6e 76 06 00 04 65 6e 76 07 00 04 65 6e
76 08 00 04 65 6e 76 09 00 04 65 6e 76 0a 00 04 65 6e 76 0b 00 04 65 6e
76 0c 00 04 65 6e 76 0d 00 04 65 6e 76 0e 00 04 65 6e 76 0f 00 03 02 01
00 0a 07 01 02 02 04 03 01 01 01 00 20 00 41 2a 0b 0b
WASM
)
    
    local boundary="----WebKitFormBoundary$(date +%s)"
    local wasm_b64=$(echo -n "$wasm_bytes" | base64 -w 0)
    local form_data="--${boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nexec-test\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n1.0.0\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nExecution test\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"component\"; filename=\"exec.wasm\"\r\nContent-Type: application/wasm\r\n\r\n${wasm_bytes}\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"resource_limits\"\r\n\r\n{\"max_memory_mb\":64,\"max_fuel\":10000000,\"epoch_deadline\":1000,\"max_table_elements\":1024,\"allowed_hosts\":[]}\r\n--${boundary}--\r\n"
    
    local upload_resp=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: multipart/form-data; boundary=----WebKitFormBoundary123" \
        --data-binary "${form_data}" \
        "${PANEL_URL}/api/wasm/upload")
    
    local component_id=$(echo "$upload_resp" | jq -r '.id')
    
    # Execute the component
    local exec_payload="{\"component_id\": \"${component_id}\", \"args\": []}"
    local response=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"component_id\": \"${component_id}\", \"args\": []}" \
        "${PANEL_URL}/api/wasm/execute")
    
    echo "$response" | grep -q '"success":true' && echo "$response" | grep -q '"exit_code":0'
}

# Test 4: List components
test_wasm_list() {
    local response=$(curl -s \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/wasm/components")
    
    echo "$response" | grep -q '\[' && echo "$response" | grep -q '"id"'
}

# Test 5: Delete component
test_wasm_delete() {
    # Upload a component to delete
    local wasm_bytes=$(cat <<'WASM' | xxd -r -p
00 61 73 6d 01 00 00 00 01 07 01 60 00 01 7f 03 02 01 00 07 07 01 03 65 6e
76 00 00 03 65 6e 76 01 00 04 65 6e 76 02 00 04 65 6e 76 03 00 04 65 6e
76 04 00 04 65 6e 76 04 00 04 65 6e 76 05 00 04 65 6e 76 06 00 04 65 6e
76 07 00 04 65 6e 76 08 00 04 65 6e 76 09 00 04 65 6e 76 0a 00 04 65 6e
76 0b 00 04 65 6e 76 0c 00 04 65 6e 76 0d 00 04 65 6e 76 0e 00 04 65 6e
76 0f 00 03 02 01 00 0a 07 01 02 02 04 03 01 01 01 00 20 00 41 2a 0b 0b
WASM
)
    
    local boundary="----WebKitFormBoundary$(date +%s)"
    local form_data="--${boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nto-delete\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n1.0.0\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nDelete test\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"component\"; filename=\"delete.wasm\"\r\nContent-Type: application/wasm\r\n\r\n$(echo -n "$wasm_bytes" | base64 -w 0)\r\n--${boundary}\r\nContent-Disposition: form-data; name=\"resource_limits\"\r\n\r\n{\"max_memory_mb\":64,\"max_fuel\":10000000,\"epoch_deadline\":1000,\"max_table_elements\":1024,\"allowed_hosts\":[]}\r\n--${boundary}--\r\n"
    
    local upload_resp=$(curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: multipart/form-data; boundary=${boundary}" \
        --data-binary "${form_data}" \
        "${PANEL_URL}/api/wasm/upload")
    
    local component_id=$(echo "$upload_resp" | jq -r '.id')
    
    # Delete the component
    local response=$(curl -s -X DELETE \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/wasm/components/${component_id}")
    
    echo "$response" | grep -q '"success":true'
}

main() {
    log_info "Starting Wasm Runtime Integration Tests"
    log_info "Panel URL: ${PANEL_URL}"
    
    if [[ -z "${ADMIN_TOKEN}" ]]; then
        log_error "ADMIN_TOKEN must be set"
        exit 1
    fi
    
    run_test "Wasm upload" test_wasm_upload
    run_test "Wasm list" test_wasm_list
    run_test "Wasm execute" test_wasm_execute
    run_test "Wasm list (again)" test_wasm_list
    run_test "Wasm delete" test_wasm_delete
    
    echo ""
    log_info "Test Summary: ${TESTS_PASSED}/${TESTS_RUN} passed, ${TESTS_FAILED} failed"
    
    if [[ ${TESTS_FAILED} -gt 0 ]]; then
        exit 1
    fi
}

main "$@"