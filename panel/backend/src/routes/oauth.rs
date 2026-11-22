// OAuth 2.1 Authorization Server for MCP and external clients
// Implements: RFC 6749 (OAuth 2.0), RFC 7636 (PKCE), RFC 8707 (Resource Indicators),
// RFC 9728 (Protected Resource Metadata), RFC 7591 (DCR), MCP Spec 2025-11-25

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json, Redirect},
    routing::{get, post},
    Form, Router,
};
use base64;
use chrono::{Duration, Utc};
use rand::{distr::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use urlencoding;

use crate::auth::{session_invalid, AuthUser};
use crate::error::{err, err_coded, internal_error, ApiError, CODE_SESSION_INVALID};
use crate::AppState;

/// Mount path: /oauth
pub fn router() -> Router<AppState> {
    Router::new()
        // Authorization endpoint (user-facing)
        .route("/authorize", get(authorize_get).post(authorize_post))
        // Token endpoint (machine-to-machine)
        .route("/token", post(token))
        // Token introspection (RFC 7662)
        .route("/introspect", post(introspect))
        // Token revocation (RFC 7009)
        .route("/revoke", post(revoke))
        // JWKS for token verification
        .route("/jwks", get(jwks))
        // Dynamic Client Registration (RFC 7591)
        .route("/register", post(register_client))
        // Client ID Metadata Document (CIMD) - MCP 2025-11-25 preferred
        .route("/.well-known/oauth-client-metadata", get(client_metadata))
        // Protected Resource Metadata (RFC 9728)
        .route("/.well-known/oauth-protected-resource", get(protected_resource_metadata))
}

/// Authorization Code Grant with PKCE (RFC 7636)
/// GET /oauth/authorize - Show consent page
async fn authorize_get(
    State(state): State<AppState>,
    Query(params): Query<AuthorizeParams>,
    AuthUser(claims): AuthUser,
) -> Result<Html<String>, ApiError> {
    // Validate client
    let client = validate_authorize_request(&state, &params).await?;

    // PKCE is mandatory per MCP spec
    if client.require_pkce && params.code_challenge.is_none() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "PKCE is required: code_challenge parameter missing (S256 only)",
        ));
    }

    if let Some(ref method) = params.code_challenge_method {
        if method != "S256" {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "Only S256 code_challenge_method is supported",
            ));
        }
    }

    // Build consent page HTML
    let html = build_consent_page(&params, &client, &claims.email);
    Ok(Html(html))
}

/// POST /oauth/authorize - Handle consent form submission
async fn authorize_post(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    headers: HeaderMap,
    Form(form): Form<AuthorizeForm>,
) -> Result<Redirect, ApiError> {
    // Re-validate the original authorize parameters from hidden form fields
    let params = AuthorizeParams {
        response_type: form.response_type,
        client_id: form.client_id,
        redirect_uri: form.redirect_uri,
        scope: form.scope,
        state: form.state,
        code_challenge: form.code_challenge,
        code_challenge_method: form.code_challenge_method,
        resource: form.resource,
    };

    let client = validate_authorize_request(&state, &params).await?;

    // Verify user consent
    if form.consent != "allow" {
        return Err(redirect_error(
            &params.redirect_uri,
            "access_denied",
            "The user denied the authorization request",
            params.state.as_deref(),
        ));
    }

    // Generate authorization code
    let code: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    let code_hash = hash_token(&code);

    // Store authorization code with PKCE challenge
    sqlx::query(
        "INSERT INTO oauth_authorization_codes (code_hash, client_id, user_id, redirect_uri, scope, code_challenge, code_challenge_method, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, NOW() + INTERVAL '10 minutes')",
    )
    .bind(&code_hash)
    .bind(client.id)
    .bind(claims.sub)
    .bind(&params.redirect_uri)
    .bind(&params.scope.unwrap_or_default())
    .bind(&params.code_challenge)
    .bind(&params.code_challenge_method)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("store auth code", e))?;

    // Redirect back to client with authorization code
    let mut redirect_url = params.redirect_uri;
    redirect_url.push_str("?code=");
    redirect_url.push_str(&code);
    if let Some(state) = params.state {
        redirect_url.push_str("&state=");
        redirect_url.push_str(&urlencoding::encode(&state));
    }
    Ok(Redirect::to(&redirect_url))
}

/// Token Endpoint - Exchange authorization code for access token (RFC 6749 §4.1.3)
async fn token(
    State(state): State<AppState>,
    Form(params): Form<TokenRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    match params.grant_type.as_str() {
        "authorization_code" => handle_authorization_code_grant(&state, params).await,
        "refresh_token" => handle_refresh_token_grant(&state, params).await,
        _ => Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Unsupported grant_type: {}", params.grant_type),
        )),
    }
}

/// Handle authorization_code grant with PKCE validation
async fn handle_authorization_code_grant(
    state: &AppState,
    params: TokenRequest,
) -> Result<Json<TokenResponse>, ApiError> {
    let code = params.code.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing code"))?;
    let code_hash = hash_token(&code);
    let client_id = params.client_id.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing client_id"))?;
    let code_verifier = params.code_verifier;

    // Look up authorization code
    let auth_code = sqlx::query!(
        "SELECT id, client_id, user_id, redirect_uri, scope, code_challenge, code_challenge_method, expires_at
         FROM oauth_authorization_codes WHERE code_hash = $1",
        &code_hash
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("lookup auth code", e))?
    .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid or expired authorization code"))?;

    // Check expiration
    if auth_code.expires_at < Utc::now() {
        // Clean up expired code
        let _ = sqlx::query("DELETE FROM oauth_authorization_codes WHERE id = $1")
            .bind(auth_code.id)
            .execute(&state.db)
            .await;
        return Err(err(StatusCode::BAD_REQUEST, "Authorization code has expired"));
    }

    // Verify client_id matches
    if auth_code.client_id != client_id {
        return Err(err(StatusCode::BAD_REQUEST, "Client ID mismatch"));
    }

    // Verify redirect_uri matches
    if params.redirect_uri.as_deref() != Some(&auth_code.redirect_uri) {
        return Err(err(StatusCode::BAD_REQUEST, "Redirect URI mismatch"));
    }

    // PKCE verification (mandatory per MCP spec)
    if auth_code.code_challenge.is_some() {
        let verifier = code_verifier.ok_or_else(|| err(StatusCode::BAD_REQUEST, "PKCE required: code_verifier missing"))?;
        let expected_challenge = match auth_code.code_challenge_method.as_deref() {
            Some("S256" | None) => {
                let mut hasher = Sha256::new();
                hasher.update(verifier.as_bytes());
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
            }
            Some(method) => return Err(err(StatusCode::BAD_REQUEST, &format!("Unsupported code_challenge_method: {}", method))),
        };
        if expected_challenge != auth_code.code_challenge.unwrap_or_default() {
            return Err(err(StatusCode::BAD_REQUEST, "PKCE verification failed: code_verifier does not match code_challenge"));
        }
    } else {
        // Should not happen because we require PKCE at authorize time, but defense in depth
        return Err(err(StatusCode::BAD_REQUEST, "PKCE required but no code_challenge was stored"));
    }

    // Consume the authorization code (single-use)
    sqlx::query("DELETE FROM oauth_authorization_codes WHERE id = $1")
        .bind(auth_code.id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete auth code", e))?;

    // Get client's api_key_id for audit attribution
    let client_api_key_id: Option<Uuid> = sqlx::query_scalar("SELECT api_key_id FROM oauth_clients WHERE id = $1")
        .bind(auth_code.client_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("lookup client api_key_id", e))?
        .flatten();

    // Issue access token (and optionally refresh token)
    let access_token: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    let access_hash = hash_token(&access_token);

    // Generate refresh token for confidential clients
    let (refresh_token, refresh_hash, refresh_expires) = {
        let client = sqlx::query!("SELECT client_secret_hash FROM oauth_clients WHERE id = $1", auth_code.client_id)
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("lookup client for refresh", e))?;

        if client.client_secret_hash.is_some() {
            let rt: String = rand::rng()
                .sample_iter(&Alphanumeric)
                .take(64)
                .map(char::from)
                .collect();
            let rth = hash_token(&rt);
            let exp = Utc::now() + Duration::days(90);
            (Some(rt), Some(rth), Some(exp))
        } else {
            (None, None, None)
        }
    };

    let access_expires = Utc::now() + Duration::hours(1);

    sqlx::query(
        "INSERT INTO oauth_tokens (access_token_hash, refresh_token_hash, client_id, user_id, scope, access_expires_at, refresh_expires_at, api_key_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&access_hash)
    .bind(&refresh_hash)
    .bind(auth_code.client_id)
    .bind(auth_code.user_id)
    .bind(&auth_code.scope)
    .bind(access_expires)
    .bind(refresh_expires)
    .bind(client_api_key_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("store tokens", e))?;

    // Build response
    let mut resp = TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        scope: if auth_code.scope.is_empty() { None } else { Some(auth_code.scope) },
        refresh_token: None,
    };
    if let Some(rt) = refresh_token {
        resp.refresh_token = Some(rt);
    }

    Ok(Json(resp))
}

/// Handle refresh_token grant
async fn handle_refresh_token_grant(
    state: &AppState,
    params: TokenRequest,
) -> Result<Json<TokenResponse>, ApiError> {
    let refresh_token = params.refresh_token.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing refresh_token"))?;
    let refresh_hash = hash_token(&refresh_token);
    let client_id = params.client_id.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing client_id"))?;

    // Look up refresh token
    let token_row = sqlx::query!(
        "SELECT id, client_id, user_id, scope, refresh_expires_at, revoked_at, api_key_id
         FROM oauth_tokens WHERE refresh_token_hash = $1",
        &refresh_hash
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("lookup refresh token", e))?
    .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid refresh token"))?;

    // Validate
    if token_row.client_id != client_id {
        return Err(err(StatusCode::BAD_REQUEST, "Client ID mismatch"));
    }
    if token_row.revoked_at.is_some() {
        return Err(err(StatusCode::BAD_REQUEST, "Refresh token has been revoked"));
    }
    if let Some(exp) = token_row.refresh_expires_at {
        if exp < Utc::now() {
            return Err(err(StatusCode::BAD_REQUEST, "Refresh token has expired"));
        }
    }

    // Rotate refresh token (revoke old, issue new)
    sqlx::query("UPDATE oauth_tokens SET revoked_at = NOW() WHERE id = $1")
        .bind(token_row.id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("revoke old refresh token", e))?;

    // Issue new tokens with the same api_key_id
    let access_token: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    let access_hash = hash_token(&access_token);

    let rt: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    let rth = hash_token(&rt);
    let refresh_expires = Utc::now() + Duration::days(90);
    let access_expires = Utc::now() + Duration::hours(1);

    sqlx::query(
        "INSERT INTO oauth_tokens (access_token_hash, refresh_token_hash, client_id, user_id, scope, access_expires_at, refresh_expires_at, api_key_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&access_hash)
    .bind(&rth)
    .bind(token_row.client_id)
    .bind(token_row.user_id)
    .bind(&token_row.scope)
    .bind(access_expires)
    .bind(refresh_expires)
    .bind(token_row.api_key_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("store rotated tokens", e))?;

    Ok(Json(TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        scope: if token_row.scope.is_empty() { None } else { Some(token_row.scope) },
        refresh_token: Some(rt),
    }))
}

/// Issue new access token (and refresh token for confidential clients)
async fn issue_tokens(
    state: &AppState,
    client_id: Uuid,
    user_id: Uuid,
    scope: &str,
) -> Result<Json<TokenResponse>, ApiError> {
    // Generate access token
    let access_token: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    let access_hash = hash_token(&access_token);

    // Get client's api_key_id for audit attribution
    let client_api_key_id: Option<Uuid> = sqlx::query_scalar("SELECT api_key_id FROM oauth_clients WHERE id = $1")
        .bind(client_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("lookup client api_key_id", e))?
        .flatten();

    // Generate refresh token for confidential clients (those with client_secret)
    let (refresh_token, refresh_hash, refresh_expires) = {
        let client = sqlx::query!("SELECT client_secret_hash FROM oauth_clients WHERE id = $1", client_id)
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("lookup client for refresh", e))?;

        if client.client_secret_hash.is_some() {
            let rt: String = rand::rng()
                .sample_iter(&Alphanumeric)
                .take(64)
                .map(char::from)
                .collect();
            let rth = hash_token(&rt);
            let exp = Utc::now() + Duration::days(90);
            (Some(rt), Some(rth), Some(exp))
        } else {
            (None, None, None)
        }
    };

    let access_expires = Utc::now() + Duration::hours(1); // 1 hour access token lifetime

    sqlx::query(
        "INSERT INTO oauth_tokens (access_token_hash, refresh_token_hash, client_id, user_id, scope, access_expires_at, refresh_expires_at, api_key_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&access_hash)
    .bind(&refresh_hash)
    .bind(client_id)
    .bind(user_id)
    .bind(scope)
    .bind(access_expires)
    .bind(refresh_expires)
    .bind(client_api_key_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("store tokens", e))?;

    // Build response
    let mut resp = TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        scope: if scope.is_empty() { None } else { Some(scope.to_string()) },
        refresh_token: None,
    };
    if let Some(rt) = refresh_token {
        resp.refresh_token = Some(rt);
    }

    Ok(Json(resp))
}

/// Token Introspection (RFC 7662)
async fn introspect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(params): Form<IntrospectRequest>,
) -> Result<Json<IntrospectResponse>, ApiError> {
    // Authenticate the introspection request (typically client credentials)
    // For simplicity, we accept the access token itself as proof of authorization
    // A production impl might require client auth here
    let token = params.token.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing token"))?;
    let token_hash = hash_token(&token);

    let token_row = sqlx::query!(
        "SELECT id, client_id, user_id, scope, access_expires_at, revoked_at, api_key_id
         FROM oauth_tokens WHERE access_token_hash = $1",
        &token_hash
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("introspect token", e))?;

    match token_row {
        Some(row) if row.revoked_at.is_none() && row.access_expires_at > Utc::now() => {
            // Active token
            let client = sqlx::query!("SELECT client_id FROM oauth_clients WHERE id = $1", row.client_id)
                .fetch_one(&state.db)
                .await
                .map_err(|e| internal_error("introspect client", e))?;

            Ok(Json(IntrospectResponse {
                active: true,
                scope: Some(row.scope),
                client_id: Some(client.client_id),
                exp: Some(row.access_expires_at.timestamp() as usize),
                iat: None, // We don't store iat separately
                sub: Some(row.user_id.to_string()),
                token_type: Some("Bearer".to_string()),
                api_key_id: row.api_key_id.map(|id| id.to_string()),
            }))
        }
        _ => Ok(Json(IntrospectResponse {
            active: false,
            scope: None,
            client_id: None,
            exp: None,
            iat: None,
            sub: None,
            token_type: None,
            api_key_id: None,
        })),
    }
}

/// Token Revocation (RFC 7009)
async fn revoke(
    State(state): State<AppState>,
    Form(params): Form<RevokeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let token = params.token.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing token"))?;
    let token_hash = hash_token(&token);

    // Try to revoke as access token
    let result = sqlx::query("UPDATE oauth_tokens SET revoked_at = NOW() WHERE access_token_hash = $1 AND revoked_at IS NULL")
        .bind(&token_hash)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("revoke access token", e))?;

    if result.rows_affected() > 0 {
        return Ok(Json(serde_json::json!({})));
    }

    // Try as refresh token
    let result = sqlx::query("UPDATE oauth_tokens SET revoked_at = NOW() WHERE refresh_token_hash = $1 AND revoked_at IS NULL")
        .bind(&token_hash)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("revoke refresh token", e))?;

    if result.rows_affected() > 0 {
        return Ok(Json(serde_json::json!({})));
    }

    // Per RFC 7009, we don't return an error if token not found
    Ok(Json(serde_json::json!({})))
}

/// JWKS Endpoint - Public keys for JWT verification
/// For OAuth 2.1, we use the same HS256 key as JWT sessions
async fn jwks(State(state): State<AppState>) -> Json<serde_json::Value> {
    // We use HS256 (symmetric) so JWKS only exposes the key ID
    // In a real deployment with asymmetric keys, this would expose the public key
    Json(serde_json::json!({
        "keys": [{
            "kty": "oct",
            "alg": "HS256",
            "kid": "dockpanel-oauth-hs256",
            "use": "sig"
        }]
    }))
}

/// Dynamic Client Registration (RFC 7591) - with redirect_uri allowlist
async fn register_client(
    State(state): State<AppState>,
    Json(params): Json<RegisterClientRequest>,
) -> Result<Json<RegisterClientResponse>, ApiError> {
    // Validate redirect URIs against allowlist
    // Allowlist: panel's own domains + localhost for development
    let allowed_origins = get_allowed_origins(&state).await?;

    for uri in &params.redirect_uris {
        if !is_redirect_uri_allowed(uri, &allowed_origins) {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!("Redirect URI not allowed: {}. Allowed origins: {:?}", uri, allowed_origins),
            ));
        }
    }

    // Generate client_id
    let client_id = format!("dpm_{}", Uuid::new_v4().simple().to_string().replace('-', ""));

    // For public clients (PKCE-only), no client_secret
    let client_secret = if params.token_endpoint_auth_method == Some("none".to_string()) {
        None
    } else {
        // Generate client_secret for confidential clients
        let secret: String = rand::rng()
            .sample_iter(&Alphanumeric)
            .take(48)
            .map(char::from)
            .collect();
        let hash = hash_token(&secret);
        Some((secret, hash))
    };

    let (secret_hash, secret_plain) = match client_secret {
        Some((plain, hash)) => (Some(hash), Some(plain)),
        None => (None, None),
    };

    let scopes = params.scope.unwrap_or_else(|| "mcp:read:*".to_string());

    sqlx::query(
        "INSERT INTO oauth_clients (client_id, client_secret_hash, client_name, client_uri, logo_uri, redirect_uris, grant_types, response_types, scope, token_endpoint_auth_method, require_pkce, allowed_origins)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(&client_id)
    .bind(&secret_hash)
    .bind(&params.client_name.unwrap_or_else(|| "MCP Client".to_string()))
    .bind(&params.client_uri)
    .bind(&params.logo_uri)
    .bind(&params.redirect_uris)
    .bind(&params.grant_types.unwrap_or_else(|| vec!["authorization_code".to_string(), "refresh_token".to_string()]))
    .bind(&params.response_types.unwrap_or_else(|| vec!["code".to_string()]))
    .bind(&scopes)
    .bind(&params.token_endpoint_auth_method.unwrap_or_else(|| "none".to_string()))
    .bind(true) // require_pkce = true (MCP mandate)
    .bind(&params.redirect_uris) // allowed_origins defaults to redirect_uris for simplicity
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("register client", e))?;

    let mut resp = RegisterClientResponse {
        client_id: client_id.clone(),
        client_id_issued_at: Utc::now().timestamp() as usize,
        client_name: params.client_name,
        client_uri: params.client_uri,
        logo_uri: params.logo_uri,
        redirect_uris: params.redirect_uris,
        grant_types: params.grant_types,
        response_types: params.response_types,
        scope: Some(scopes),
        token_endpoint_auth_method: params.token_endpoint_auth_method,
        require_pkce: Some(true),
    };

    if let Some(secret) = secret_plain {
        resp.client_secret = Some(secret);
        resp.client_secret_expires_at = Some(0); // 0 = never expires
    }

    Ok(Json(resp))
}

/// Client ID Metadata Document (CIMD) - MCP 2025-11-25 preferred registration
/// Served at /.well-known/oauth-client-metadata
async fn client_metadata(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // This serves the panel's own client metadata for MCP clients
    // that want to register via CIMD instead of DCR
    let base_url = get_base_url(&state).await?;

    Ok(Json(serde_json::json!({
        "client_id": "dockpanel-mcp-server",
        "client_name": "DockPanel MCP Server",
        "redirect_uris": ["http://127.0.0.1:3081/callback", "http://localhost:3081/callback"],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "scope": "mcp:read:*",
        "token_endpoint_auth_method": "none",
        "require_pkce": true,
        "token_endpoint": format!("{}/oauth/token", base_url),
        "authorization_endpoint": format!("{}/oauth/authorize", base_url),
        "introspection_endpoint": format!("{}/oauth/introspect", base_url),
        "revocation_endpoint": format!("{}/oauth/revoke", base_url),
        "jwks_uri": format!("{}/oauth/jwks", base_url),
    }))
}

/// Protected Resource Metadata (RFC 9728)
/// Served at /.well-known/oauth-protected-resource
async fn protected_resource_metadata(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let base_url = get_base_url(&state).await?;

    Ok(Json(serde_json::json!({
        "resource": base_url,
        "authorization_servers": [format!("{}/oauth", base_url)],
        "jwks_uri": format!("{}/oauth/jwks", base_url),
        "scopes_supported": ["mcp:read:*", "mcp:write:*", "mcp:admin:*"],
        "bearer_methods_supported": ["header"],
        "resource_documentation": "https://docs.dockpanel.dev/mcp"
    }))
}

/// Validate authorization request parameters
async fn validate_authorize_request(
    state: &AppState,
    params: &AuthorizeParams,
) -> Result<OAuthClient, ApiError> {
    if params.response_type != "code" {
        return Err(err(StatusCode::BAD_REQUEST, "Unsupported response_type. Only 'code' is supported."));
    }

    let client_id = params.client_id.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing client_id"))?;

    let client = sqlx::query_as!(
        OAuthClient,
        "SELECT id, client_id, client_secret_hash, client_name, redirect_uris, scope, token_endpoint_auth_method, require_pkce, allowed_origins
         FROM oauth_clients WHERE client_id = $1",
        client_id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("lookup client", e))?
    .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid client_id"))?;

    // Validate redirect_uri against client's allowlist
    let redirect_uri = params.redirect_uri.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing redirect_uri"))?;
    if !client.redirect_uris.iter().any(|u| u == redirect_uri) {
        return Err(err(StatusCode::BAD_REQUEST, "Redirect URI not registered for this client"));
    }

    // Validate scope (subset of client's allowed scopes)
    if let Some(req_scope) = &params.scope {
        let client_scopes: std::collections::HashSet<&str> = client.scope.split_whitespace().collect();
        for s in req_scope.split_whitespace() {
            if !client_scopes.contains(s) {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Scope not allowed: {}", s)));
            }
        }
    }

    // Validate resource indicator (RFC 8707) - optional but recommended
    if let Some(resource) = &params.resource {
        // For DockPanel, the resource is the panel's base URL
        let base_url = get_base_url(state).await?;
        if resource != &base_url && resource != &format!("{}/", base_url) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid resource indicator"));
        }
    }

    Ok(client)
}

/// Build consent page HTML
fn build_consent_page(params: &AuthorizeParams, client: &OAuthClient, user_email: &str) -> String {
    let redirect_uri = params.redirect_uri.as_deref().unwrap_or("");
    let scope = params.scope.as_deref().unwrap_or("");
    let state = params.state.as_deref().unwrap_or("");
    let client_id = &client.client_id;

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Authorize Application</title>
    <style>
        body {{ font-family: system-ui, sans-serif; max-width: 480px; margin: 2rem auto; padding: 1rem; }}
        .card {{ border: 1px solid #e5e7eb; border-radius: 8px; padding: 1.5rem; }}
        h1 {{ font-size: 1.25rem; margin-bottom: 1rem; }}
        .info {{ background: #f9fafb; padding: 1rem; border-radius: 6px; margin: 1rem 0; font-size: 0.875rem; }}
        .btn-group {{ display: flex; gap: 0.5rem; margin-top: 1.5rem; }}
        button {{ padding: 0.625rem 1.25rem; border-radius: 6px; font-size: 0.875rem; font-weight: 500; cursor: pointer; border: none; }}
        .btn-allow {{ background: #16a34a; color: white; }}
        .btn-deny {{ background: #dc2626; color: white; }}
        input[type="hidden"] {{ display: none; }}
    </style>
</head>
<body>
    <div class="card">
        <h1>Authorize Application</h1>
        <p><strong>{}</strong> wants to access your DockPanel account.</p>
        <div class="info">
            <p><strong>Email:</strong> {}</p>
            <p><strong>Application:</strong> {} ({})</p>
            <p><strong>Requested scopes:</strong> {}</p>
        </div>
        <form method="POST" action="/oauth/authorize">
            <input type="hidden" name="response_type" value="code">
            <input type="hidden" name="client_id" value="{}">
            <input type="hidden" name="redirect_uri" value="{}">
            <input type="hidden" name="scope" value="{}">
            <input type="hidden" name="state" value="{}">
            <input type="hidden" name="code_challenge" value="{}">
            <input type="hidden" name="code_challenge_method" value="{}">
            <input type="hidden" name="resource" value="{}">
            <div class="btn-group">
                <button type="submit" name="consent" value="allow" class="btn-allow">Allow</button>
                <button type="submit" name="consent" value="deny" class="btn-deny">Deny</button>
            </div>
        </form>
    </div>
</body>
</html>"#,
        client.client_name,
        user_email,
        client.client_name,
        client_id,
        if scope.is_empty() { "mcp:read:* (default)" } else { scope },
        client_id,
        redirect_uri,
        scope,
        state,
        params.code_challenge.as_deref().unwrap_or(""),
        params.code_challenge_method.as_deref().unwrap_or(""),
        params.resource.as_deref().unwrap_or("")
    )
}

/// Build error redirect
fn redirect_error(redirect_uri: &str, error: &str, description: &str, state: Option<&str>) -> ApiError {
    let mut url = redirect_uri.to_string();
    url.push_str("?error=");
    url.push_str(&urlencoding::encode(error));
    url.push_str("&error_description=");
    url.push_str(&urlencoding::encode(description));
    if let Some(s) = state {
        url.push_str("&state=");
        url.push_str(&urlencoding::encode(s));
    }
    err(StatusCode::FOUND, &url) // Using FOUND (302) for redirect
}

// Request/Response types

#[derive(Debug, Deserialize)]
struct AuthorizeParams {
    response_type: String,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    resource: Option<String>, // RFC 8707 Resource Indicators
}

#[derive(Debug, Deserialize)]
struct AuthorizeForm {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    scope: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    resource: Option<String>,
    consent: String, // "allow" or "deny"
}

#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    scope: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IntrospectRequest {
    token: Option<String>,
    token_type_hint: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

#[derive(Debug, Serialize)]
struct IntrospectResponse {
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exp: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iat: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RevokeRequest {
    token: Option<String>,
    token_type_hint: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegisterClientRequest {
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    client_uri: Option<String>,
    logo_uri: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    scope: Option<String>,
    token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Serialize)]
struct RegisterClientResponse {
    client_id: String,
    client_id_issued_at: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_uri: Option<String>,
    redirect_uris: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grant_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_endpoint_auth_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_secret_expires_at: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_pkce: Option<bool>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct OAuthClient {
    id: Uuid,
    client_id: String,
    client_secret_hash: Option<String>,
    client_name: String,
    redirect_uris: Vec<String>,
    scope: String,
    token_endpoint_auth_method: String,
    require_pkce: bool,
    allowed_origins: Vec<String>,
}

// Helper functions

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

async fn get_allowed_origins(state: &AppState) -> Result<Vec<String>, ApiError> {
    // Get panel's base URL from settings
    let base_url: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'panel_url'")
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("get panel_url", e))?;

    let mut origins = vec!["http://localhost:3081".to_string(), "http://127.0.0.1:3081".to_string()];
    if let Some(url) = base_url {
        origins.push(url);
    }
    Ok(origins)
}

fn is_redirect_uri_allowed(uri: &str, allowed_origins: &[String]) -> bool {
    for origin in allowed_origins {
        if uri.starts_with(origin) {
            return true;
        }
    }
    false
}

async fn get_base_url(state: &AppState) -> Result<String, ApiError> {
    let base_url: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'panel_url'")
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("get panel_url", e))?;

    base_url.ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "panel_url not configured"))
}