-- Policy Engine settings
INSERT INTO settings (key, value) VALUES
    ('policy_engine_enabled', 'false'),
    ('policy_engine_opa_url', ''),
    ('policy_engine_bundled_policies_path', '/etc/dockpanel/policies/bundled'),
    ('policy_engine_custom_policies_path', '/etc/dockpanel/policies/custom'),
    ('policy_engine_default_decision', 'allow'),
    ('policy_engine_audit_mode', 'true')
ON CONFLICT (key) DO NOTHING;