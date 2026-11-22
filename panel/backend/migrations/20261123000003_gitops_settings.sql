-- GitOps Controller settings
INSERT INTO settings (key, value) VALUES
    ('gitops_enabled', 'false'),
    ('gitops_git_repo_url', ''),
    ('gitops_git_branch', 'main'),
    ('gitops_git_ssh_key_path', ''),
    ('gitops_sync_interval_seconds', '300'),
    ('gitops_dry_run', 'true'),
    ('gitops_auto_reconcile', 'false'),
    ('gitops_prune_orphaned', 'false')
ON CONFLICT (key) DO NOTHING;