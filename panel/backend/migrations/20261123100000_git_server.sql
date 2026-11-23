-- Git Server: built-in git repositories with smart-HTTP transport
-- Tables: git_repos, git_repo_members, git_tokens, git_pull_requests,
--         git_pr_reviews, git_pr_comments, git_issues, git_issue_comments

CREATE TABLE git_repos (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    owner_name TEXT NOT NULL,
    slug TEXT NOT NULL,
    description TEXT DEFAULT '',
    visibility TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'public')),
    default_branch TEXT NOT NULL DEFAULT 'main',
    protect_default BOOLEAN NOT NULL DEFAULT false,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (owner_name, slug)
);

CREATE INDEX idx_git_repos_user_id ON git_repos(user_id);
CREATE INDEX idx_git_repos_visibility ON git_repos(visibility);

CREATE TABLE git_repo_members (
    repo_id UUID NOT NULL REFERENCES git_repos(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'read' CHECK (role IN ('admin', 'write', 'read')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repo_id, user_id)
);

CREATE INDEX idx_git_repo_members_user_id ON git_repo_members(user_id);

CREATE TABLE git_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    token_prefix TEXT NOT NULL,
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE git_pull_requests (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    repo_id UUID NOT NULL REFERENCES git_repos(id) ON DELETE CASCADE,
    number BIGINT NOT NULL,
    title TEXT NOT NULL,
    body TEXT DEFAULT '',
    author_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_ref TEXT NOT NULL,
    target_ref TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'merged', 'closed')),
    merged_commit TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (repo_id, number)
);

CREATE INDEX idx_git_pull_requests_repo_id ON git_pull_requests(repo_id);
CREATE INDEX idx_git_pull_requests_state ON git_pull_requests(state);

CREATE TABLE git_pr_reviews (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    pr_id UUID NOT NULL REFERENCES git_pull_requests(id) ON DELETE CASCADE,
    reviewer_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('approved', 'changes_requested', 'commented')),
    body TEXT DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (pr_id, reviewer_id)
);

CREATE INDEX idx_git_pr_reviews_pr_id ON git_pr_reviews(pr_id);

CREATE TABLE git_pr_comments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    pr_id UUID NOT NULL REFERENCES git_pull_requests(id) ON DELETE CASCADE,
    author_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    commit_sha TEXT,
    file_path TEXT,
    line INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_git_pr_comments_pr_id ON git_pr_comments(pr_id);

CREATE TABLE git_issues (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    repo_id UUID NOT NULL REFERENCES git_repos(id) ON DELETE CASCADE,
    number BIGINT NOT NULL,
    title TEXT NOT NULL,
    body TEXT DEFAULT '',
    author_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'closed')),
    closed_by_commit TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (repo_id, number)
);

CREATE INDEX idx_git_issues_repo_id ON git_issues(repo_id);
CREATE INDEX idx_git_issues_state ON git_issues(state);

CREATE TABLE git_issue_comments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    issue_id UUID NOT NULL REFERENCES git_issues(id) ON DELETE CASCADE,
    author_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_git_issue_comments_issue_id ON git_issue_comments(issue_id);

-- Trigger to auto-update updated_at on git_repos
CREATE TRIGGER update_git_repos_updated_at
    BEFORE UPDATE ON git_repos
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Trigger to auto-update updated_at on git_pull_requests
CREATE TRIGGER update_git_pull_requests_updated_at
    BEFORE UPDATE ON git_pull_requests
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Trigger to auto-update updated_at on git_issues
CREATE TRIGGER update_git_issues_updated_at
    BEFORE UPDATE ON git_issues
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();