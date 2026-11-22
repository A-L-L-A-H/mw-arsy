-- MCP OAuth 2.1 Implementation
-- Provides OAuth 2.1 Authorization Server for MCP server authentication
-- Includes PKCE support, DCR, CIMD, token introspection/revocation

-- OAuth Clients table (for MCP clients and other OAuth consumers)
CREATE TABLE oauth_clients (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    client_id VARCHAR(255) NOT NULL UNIQUE,
    client_secret_hash TEXT, -- NULL for public clients (PKCE-only)
    client_name VARCHAR(255) NOT NULL,
    client_uri TEXT,
    logo_uri TEXT,
    redirect_uris TEXT[] NOT NULL DEFAULT '{}', -- allowlist of redirect URIs
    grant_types TEXT[] NOT NULL DEFAULT '{"authorization_code", "refresh_token"}',
    response_types TEXT[] NOT NULL DEFAULT '{"code"}',
    scope TEXT NOT NULL DEFAULT '', -- space-separated scopes
    token_endpoint_auth_method VARCHAR(50) NOT NULL DEFAULT 'none', -- 'none', 'client_secret_basic', 'client_secret_post'
    require_pkce BOOLEAN NOT NULL DEFAULT TRUE, -- MCP spec mandates PKCE
    allowed_origins TEXT[] NOT NULL DEFAULT '{}', -- for CORS on authorization endpoint
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Links to api_keys for audit attribution when MCP server uses a dp_ key
    api_key_id UUID REFERENCES api_keys(id) ON DELETE SET NULL
);

CREATE INDEX idx_oauth_clients_client_id ON oauth_clients(client_id);
CREATE INDEX idx_oauth_clients_api_key ON oauth_clients(api_key_id);

-- OAuth Authorization Codes (short-lived, single-use, PKCE-bound)
CREATE TABLE oauth_authorization_codes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code_hash TEXT NOT NULL UNIQUE, -- SHA256 of the authorization code
    client_id UUID NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT '',
    code_challenge TEXT, -- PKCE code challenge
    code_challenge_method VARCHAR(20), -- 'S256' (only method we accept)
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_oauth_auth_codes_client ON oauth_authorization_codes(client_id);
CREATE INDEX idx_oauth_auth_codes_user ON oauth_authorization_codes(user_id);
CREATE INDEX idx_oauth_auth_codes_expires ON oauth_authorization_codes(expires_at);

-- OAuth Access Tokens and Refresh Tokens
CREATE TABLE oauth_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    access_token_hash TEXT NOT NULL UNIQUE, -- SHA256 of the access token
    refresh_token_hash TEXT UNIQUE, -- SHA256 of the refresh token (optional)
    client_id UUID NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scope TEXT NOT NULL DEFAULT '',
    access_expires_at TIMESTAMPTZ NOT NULL,
    refresh_expires_at TIMESTAMPTZ, -- NULL if no refresh token issued
    revoked_at TIMESTAMPTZ, -- for token revocation
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_oauth_tokens_client ON oauth_tokens(client_id);
CREATE INDEX idx_oauth_tokens_user ON oauth_tokens(user_id);
CREATE INDEX idx_oauth_tokens_access_expires ON oauth_tokens(access_expires_at);
CREATE INDEX idx_oauth_tokens_refresh_expires ON oauth_tokens(refresh_expires_at);
CREATE INDEX idx_oauth_tokens_revoked ON oauth_tokens(revoked_at);

-- Pre-populate with a default MCP client configuration
-- This client represents the MCP server itself when it uses a dp_ key
-- The actual MCP clients (Claude Desktop, etc.) register via DCR or CIMD
INSERT INTO oauth_clients (client_id, client_name, redirect_uris, scope, token_endpoint_auth_method, require_pkce, allowed_origins)
VALUES (
    'dockpanel-mcp-server',
    'DockPanel MCP Server (built-in)',
    ARRAY['http://127.0.0.1:3081/callback', 'http://localhost:3081/callback'],
    'mcp:read:*',
    'none',
    TRUE,
    ARRAY['http://127.0.0.1:3081', 'http://localhost:3081']
) ON CONFLICT (client_id) DO NOTHING;