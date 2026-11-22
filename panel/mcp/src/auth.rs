// OAuth 2.1 token validation for MCP server
// Validates tokens against the panel's OAuth 2.1 introspection endpoint
// Supports both legacy static token (for backward compat) and OAuth 2.1 tokens

use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::backend_client;

#[allow(dead_code)]
const MCP_ENV_PATH: &str = "/etc/dockpanel/mcp.env";

/// Token introspection response from panel's OAuth server
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fields used in serialization
struct IntrospectResponse {
    active: bool,
    scope: Option<String>,
    client_id: Option<String>,
    exp: Option<usize>,
    sub: Option<String>,
    token_type: Option<String>,
}

/// Load the panel API base URL from the same env file the API uses
fn panel_api_base_url() -> Result<String, String> {
    let env = std::fs::read_to_string("/etc/dockpanel/api.env")
        .map_err(|e| format!("Cannot read /etc/dockpanel/api.env: {e}"))?;
    let addr = env
        .lines()
        .find_map(|l| l.trim().strip_prefix("LISTEN_ADDR="))
        .ok_or_else(|| "/etc/dockpanel/api.env has no LISTEN_ADDR".to_string())?
        .trim();
    Ok(format!("http://{addr}"))
}

/// Validate OAuth 2.1 token via panel's introspection endpoint
async fn validate_oauth_token(token: &str) -> Result<IntrospectResponse, String> {
    let base_url = panel_api_base_url()?;
    let url = format!("{base_url}/oauth/introspect");

    let client = Client::new();
    let resp = client
        .post(&url)
        .form(&[("token", token)])
        .send()
        .await
        .map_err(|e| format!("Cannot reach panel OAuth introspection endpoint: {e}"))?;

    let status = resp.status();
    let body: IntrospectResponse = resp
        .json()
        .await
        .map_err(|e| format!("Invalid JSON from introspection endpoint: {e}"))?;

    if !status.is_success() {
        return Err(format!("Introspection endpoint returned {status}"));
    }

    if !body.active {
        return Err("Token is not active (expired, revoked, or invalid)".to_string());
    }

    // Verify token type is Bearer
    if body.token_type.as_deref() != Some("Bearer") {
        return Err("Invalid token type".to_string());
    }

    Ok(body)
}

/// Check if the legacy static token is configured and matches
fn check_legacy_token(provided: &str) -> bool {
    backend_client::load_token()
        .map(|expected| {
            let (a, b) = (provided.as_bytes(), expected.as_bytes());
            if a.len() != b.len() {
                return false;
            }
            a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
        })
        .unwrap_or(false)
}

/// Main authentication middleware for MCP server
/// Tries OAuth 2.1 token validation first, falls back to legacy static token
pub async fn require_bearer_token(req: Request, next: Next) -> Response {
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let Some(token) = provided else {
        return (StatusCode::UNAUTHORIZED, "Missing Authorization header with Bearer token").into_response();
    };

    // Try OAuth 2.1 token validation first
    match validate_oauth_token(token).await {
        Ok(introspection) => {
            debug!(
                "MCP request authenticated via OAuth 2.1: client_id={:?}, scope={:?}, sub={:?}",
                introspection.client_id, introspection.scope, introspection.sub
            );

            // Verify scope includes mcp:read:* (required for all MCP tools)
            let has_mcp_read = introspection
                .scope
                .as_deref()
                .map(|s| s.split_whitespace().any(|sc| sc == "mcp:read:*" || sc.starts_with("mcp:read:") || sc == "mcp:admin:*"))
                .unwrap_or(false);

            if !has_mcp_read {
                warn!("MCP request rejected: insufficient scope (has {:?})", introspection.scope);
                return (StatusCode::FORBIDDEN, "Token lacks required MCP scope (mcp:read:*)").into_response();
            }

            // Store introspection info in request extensions for downstream use
            // (e.g., for audit attribution we'd want the client_id/user_id)
            // For now, just allow the request through
            next.run(req).await
        }
        Err(oauth_err) => {
            debug!("OAuth 2.1 validation failed: {oauth_err}; trying legacy static token");

            // Fallback to legacy static token for backward compatibility
            if check_legacy_token(token) {
                debug!("MCP request authenticated via legacy static token");
                next.run(req).await
            } else {
                warn!("MCP request rejected: both OAuth 2.1 and legacy token validation failed");
                (StatusCode::UNAUTHORIZED, "Missing or invalid bearer token").into_response()
            }
        }
    }
}