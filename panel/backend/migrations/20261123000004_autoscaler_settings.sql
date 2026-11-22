-- Horizontal Autoscaler settings
INSERT INTO settings (key, value) VALUES
    ('autoscaler_enabled', 'false'),
    ('autoscaler_keda_enabled', 'false'),
    ('autoscaler_fallback_enabled', 'true'),
    ('autoscaler_min_replicas', '1'),
    ('autoscaler_max_replicas', '10'),
    ('autoscaler_cooldown_seconds', '300'),
    ('autoscaler_metrics_port', '9090')
ON CONFLICT (key) DO NOTHING;