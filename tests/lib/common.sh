#!/usr/bin/env bash
# Common library for integration tests

# Check for required commands
command -v curl >/dev/null 2>&1 || { echo "curl is required but not installed. Aborting." >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq is required but not installed. Aborting." >&2; exit 1; }

# Default panel URL
PANEL_URL="${PANEL_URL:-https://panel.example.com}"

# Function to wait for a condition
wait_for() {
    local timeout="${1:-30}"
    local interval="${2:-2}"
    local condition="${3}"
    local elapsed=0
    
    while [[ ${elapsed} -lt ${timeout} ]]; do
        if eval "${condition}"; then
            return 0
        fi
        sleep "${interval}"
        elapsed=$((elapsed + interval))
    done
    return 1
}

# Function to create a test site
create_test_site() {
    local domain="${1:-test.example.com}"
    local runtime="${2:-static}"
    local payload="{\"domain\": \"${domain}\", \"runtime\": \"${runtime}\"}"
    
    curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "${payload}" \
        "${PANEL_URL}/api/sites"
}

# Function to create a test database
create_test_database() {
    local name="${1:-testdb}"
    local engine="${2:-postgresql}"
    local site_id="${3}"
    local payload="{\"name\": \"${name}\", \"engine\": \"${engine}\"}"
    
    if [[ -n "${site_id}" ]]; then
        payload="{\"name\": \"${name}\", \"engine\": \"${engine}\", \"site_id\": \"${site_id}\"}"
    fi
    
    curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "${payload}" \
        "${PANEL_URL}/api/databases"
}

# Function to create a test app
create_test_app() {
    local name="${1:-testapp}"
    local image="${2:-nginx:alpine}"
    local payload="{\"name\": \"${name}\", \"image\": \"${image}\", \"container_port\": 80}"
    
    curl -s -X POST \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "${payload}" \
        "${PANEL_URL}/api/apps"
}

# Function to clean up test resources
cleanup_test_resources() {
    # Clean up test sites
    curl -s -X DELETE \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/sites" 2>/dev/null || true
    
    # Clean up test apps
    curl -s -X DELETE \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/apps" 2>/dev/null || true
    
    # Clean up test databases
    curl -s -X DELETE \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        "${PANEL_URL}/api/databases" 2>/dev/null || true
}

# Trap cleanup on exit
trap cleanup_test_resources EXIT