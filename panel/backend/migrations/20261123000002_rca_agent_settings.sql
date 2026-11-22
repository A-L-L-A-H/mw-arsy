-- RCA Agent settings
INSERT INTO settings (key, value) VALUES
    ('rca_enabled', 'false'),
    ('rca_mcp_url', 'http://127.0.0.1:3081/mcp'),
    ('rca_mcp_api_key', ''),
    ('rca_auto_trigger_alert', 'true'),
    ('rca_auto_trigger_deploy', 'true'),
    ('rca_max_history', '100'),
    ('rca_llm_provider', 'anthropic'),
    ('rca_llm_model', 'claude-3-5-sonnet-20241022'),
    ('rca_llm_api_key', '')
ON CONFLICT (key) DO NOTHING;