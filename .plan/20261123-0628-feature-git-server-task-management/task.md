# Task List — Built-in Git Server + Task Management (DockPanel fork)

> Source blueprint: [spec]../../20261123-1430-review-dockpanel-comprehensive-audit/../../../../repo/.plan/20261123-0628-feature-git-server-task-management/spec.md
>
> Execution order: Phase 0 → 1 → 2 → 3 → 4 → 5. Phases 1–5 may overlap once their
> dependencies are done, but within a phase tasks run in order.
>
> Every task below is atomic (≤ 3 files unless explicitly larger), imperative, and
> ends with acceptance criteria. An implementer agent must be able to execute any
> task from this file plus `rule.md` and `spec.md` alone.
>
> **Context files to read before any task:** `rule.md`, `spec.md`, `design.md`,
> `panel/backend/src/routes/git_deploys.rs` (trigger contract + request shape),
> `panel/backend/src/services/agent.rs` (AgentClient), `panel/agent/src/safe_cmd.rs`
> (spawn discipline), `panel/backend/src/routes/mod.rs` (router merge pattern),
> `panel/frontend/src/main.tsx` (route registration).

---

## Phase 0 — Foundations (auth + spawn discipline)

- [ ] **T-P0-01 · [S]** Create the git-server token infrastructure.
  Files: `panel/backend/src/services/git_tokens.rs` (new), `panel/backend/src/services/mod.rs` (edit).
  1. Add module `git_tokens` to `services/mod.rs`.
  2. In `git_tokens.rs` implement `pub fn generate_token() -> String` producing
     `dpgit_` + 40 hex chars (random via `rand::rngs::OsRng`), mirroring the
     `dp_` prefix discipline used by `api_keys.rs` (SHA-256 hex hashing with
     `sha2::{Sha256, Digest}` + `hex::encode`).
  3. Implement `pub fn hash_token(raw: &str) -> String` (SHA-256 hex, same as
     api_keys.rs lines 6/76).
  4. Unit test in the same file (`#[cfg(test)]`): generate → hash → hash matches,
     prefix present, length correct.
  Accept: `cargo test -p dockpanel-backend git_tokens` passes; no DB access needed.
  Depends on: nothing.

- [ ] **T-P0-02 · [S]** Extend the basic-auth extractor to accept git tokens.
  Files: `panel/backend/src/auth.rs` (edit), `panel/backend/src/routes/auth.rs` (edit only if `AuthUser` lives there — check with grep before editing).
  1. Locate the basic-auth parsing path (grep `Authorization` + `basic` in
     `backend/src`).
  2. Add a branch: when a Basic credential does NOT decode to `email:password`,
     try it as a raw `dpgit_` token against `git_tokens.token_hash` (same
     SHA-256 comparison as `api_keys`, constant-time via `subtle` if that is
     what api_keys.rs uses — mirror it exactly).
  3. On match: load user, update `git_tokens.last_used_at`, return the same
     authenticated principal type the password path returns.
  4. On no match anywhere: identical 401 behavior as today (no new error shape).
  Accept: a pin test (T-P5-03) exercises this; for this task, `cargo check`
  passes and the existing auth pin tests still pass.
  Depends on: T-P0-01.

- [ ] **T-P0-03 · [M]** Write the spawn-discipline pin test for git processes.
  Files: `tests/git-spawn-discipline-pin-e2e.sh` (new).
  1. Read `panel/agent/src/safe_cmd.rs` and its existing tests.
  2. Write the pin test asserting: every `git upload-pack` / `git receive-pack`
     / `git` plumbing call site in `panel/agent/src/services/git_http.rs`
     (created later, T-P1-02) MUST route through `safe_cmd` (env_clear).
     Structure it like `tests/nginx-listen-pin-e2e.sh`: grep the file, fail on
     `Command::new("git")` that is not inside a safe_cmd wrapper.
  3. Add the test to the regression-pins CI list the same way other pin tests
     are registered (grep `regression-pins.yml` for the registration pattern).
  Accept: `bash tests/git-spawn-discipline-pin-e2e.sh` exits 0 today (file
  does not exist yet → skip logic like other pins) and will fail if a future
  diff introduces a raw spawn.
  Depends on: nothing.

---

## Phase 1 — Git server core (agent + backend)

- [ ] **T-P1-01 · [M]** Agent: git repos service module.
  Files: `panel/agent/src/services/git_repos.rs` (new), `panel/agent/src/services/mod.rs` (edit).
  1. Add module registration.
  2. Implement:
     - `pub fn repo_path(root: &Path, owner: &str, slug: &str) -> PathBuf`
       → `<root>/<owner>/<slug>.git`, with path-traversal guard (reject any
       component containing `/`, `\`, `..`, or non-[a-z0-9-_]).
     - `pub fn init_repo(root, owner, slug) -> Result<()>` — create dirs,
       `git init --bare` (via safe_cmd), write a default `description`,
       set `http.receivepack=true` in config (stateless-rpc needs it).
     - `pub fn delete_repo(root, owner, slug) -> Result<()>` — refuse if
       resolved path is not inside root (defense in depth).
     - `pub fn repo_stats(root, owner, slug) -> Result<RepoStats>` — branches,
       tags, HEAD commit, size on disk (du via safe_cmd or walkdir).
  3. Unit tests: traversal guard rejects `../../etc`, valid slug passes,
     init+delete round-trip on a tempdir.
  Accept: `cargo test -p dockpanel-agent git_repos` passes.
  Depends on: nothing.

- [ ] **T-P1-02 · [M]** Agent: smart-HTTP git transport endpoints.
  Files: `panel/agent/src/services/git_http.rs` (new), `panel/agent/src/routes/mod.rs` (edit), `panel/agent/src/main.rs` (edit, router line).
  1. Implement Axum handlers:
     - `GET  /agent/git/info/refs?service=git-upload-pack|git-receive-pack`
     - `POST /agent/git/git-upload-pack`
     - `POST /agent/git/git-receive-pack`
     following the git smart-HTTP protocol exactly (content-type
     `application/x-git-{service}-advertisement` for GET,
     `application/x-git-{service}-result` for POST; pkt-line `# service=` header;
     flush-pkt `0000`).
  2. Spawn `git upload-pack --stateless-rpc <path>` / `git receive-pack
     --stateless-rpc <path>` via `safe_cmd`, streaming request body → stdin and
     stdout → response body (Axum `Body::from_stream`; reqwest/axum both support
     streaming — no full buffering for push).
  3. Path resolution goes through `git_repos::repo_path` (traversal guard).
  4. `receive-pack` additionally: after a successful push, parse the ref-update
     lines from the request (pre-computed in the pkt-line) and return them so
     the backend can build push notifications; do NOT parse inside receive-pack
     itself beyond what the protocol requires.
  5. Wire the three routes into the agent router (mirror how `terminal.rs`
     registers).
  Accept: with a local agent running and a repo created by T-P1-01, `git clone
  http://127.0.0.1:<port>/agent/git/<owner>/<slug>.git` works with a token
  (temporary stub auth in this task, replaced in T-P1-05).
  Depends on: T-P0-01 (token format), T-P1-01.

- [ ] **T-P1-03 · [M]** Agent: repo management + refs/commits read endpoints.
  Files: `panel/agent/src/routes/git_repos.rs` (new), `panel/agent/src/routes/mod.rs` (edit).
  1. Endpoints (all via safe_cmd git plumbing, JSON responses):
     - `POST   /agent/repos/:owner/:slug`            → init (from T-P1-01)
     - `DELETE /agent/repos/:owner/:slug`            → delete
     - `GET    /agent/repos/:owner/:slug/stats`      → RepoStats
     - `GET    /agent/repos/:owner/:slug/branches`   → `git for-each-ref refs/heads --format='%(refname:short)%00%(objectname)%00%(committerdate:iso8601)'`
     - `GET    /agent/repos/:owner/:slug/commits?ref=&limit=&skip=` → `git log --format` with `%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%b` parsing
     - `GET    /agent/repos/:owner/:slug/commit/:sha` → `git show --stat` + patch (cap patch size, e.g. 512 KB, truncate marker)
     - `GET    /agent/repos/:owner/:slug/tree?ref=&path=` → `git ls-tree`
     - `GET    /agent/repos/:owner/:slug/raw?ref=&path=` → `git cat-file blob` (size cap)
     - `GET    /agent/repos/:owner/:slug/diff/:a...:b` → `git diff a..b` (cap size)
  2. Every handler validates owner/slug through the traversal guard first.
  3. Wire into the agent router.
  Accept: `cargo build -p dockpanel-agent` passes; manual curl against a local
  agent on a test repo returns expected shapes.
  Depends on: T-P1-01.

- [ ] **T-P1-04 · [M]** Backend: schema migrations for git server.
  Files: `panel/backend/migrations/20261123100000_git_server.sql` (new).
  1. Tables (naming convention follows existing migrations — snake_case, UUID
     PKs via gen_random_uuid() where the codebase uses it, TIMESTAMPTZ):
     - `git_repos (id, user_id FK, owner_name, slug, description, visibility
       ('private'|'public'), default_branch, size_bytes, created_at, updated_at,
       UNIQUE(owner_name, slug))`
     - `git_repo_members (repo_id FK, user_id FK, role ('admin'|'write'|'read'),
       created_at, UNIQUE(repo_id, user_id))`
     - `git_tokens (id, user_id FK, name, token_hash UNIQUE, token_prefix,
       last_used_at, created_at)` — hash via T-P0-01 discipline
     - `git_pull_requests (id, repo_id FK, number BIGINT, title, body,
       author_id FK, source_ref, target_ref, state ('open'|'merged'|'closed'),
       merged_commit, created_at, updated_at, UNIQUE(repo_id, number))`
     - `git_pr_reviews (id, pr_id FK, reviewer_id FK, state
       ('approved'|'changes_requested'|'commented'), body, created_at,
       UNIQUE(pr_id, reviewer_id))` — one decisive review per reviewer
     - `git_pr_comments (id, pr_id FK, author_id FK, body, commit_sha NULL,
       file_path NULL, line NULL, created_at)`
     - `git_issues (id, repo_id FK, number BIGINT, title, body, author_id FK,
       state ('open'|'closed'), closed_by_commit NULL, created_at, updated_at,
       UNIQUE(repo_id, number))`
     - `git_issue_comments (id, issue_id FK, author_id FK, body, created_at)`
  2. Indexes: FK columns, `git_repos(owner_name, slug)`, issues/PRs by repo+state,
     comments by parent.
  3. `git_repos.owner_name` is denormalized (the owner's slug as a string), so a
     repo's URL never breaks if the owner row is renamed.
  4. Run `sqlx migrate run` (or the repo's canonical command) locally; it must
     apply clean on a fresh PostgreSQL 16.
  Accept: migration applies and reverses conceptually; `cargo check -p dockpanel-backend`
  still passes (sqlx macros will pick the schema up if offline data needs a refresh —
  run the repo's sqlx prepare step if used).
  Depends on: nothing.

- [ ] **T-P1-05 · [M]** Backend: repo CRUD + token auth enforcement on transport.
  Files: `panel/backend/src/routes/git_repos.rs` (new), `panel/backend/src/routes/mod.rs` (edit), `panel/backend/src/main.rs` (edit if router registration is there).
  1. REST endpoints (JWT or `dpgit_` token from T-P0-02):
     - `POST /api/repos`            create (owner = caller's owner_name; slug validation)
     - `GET  /api/repos`            list mine (+ public repos of others)
     - `GET  /api/repos/:owner/:slug`        detail + stats (proxied from agent
       via `AgentClient`, pattern from `services/agent.rs`)
     - `PATCH /api/repos/:owner/:slug`       description, visibility, default branch
     - `DELETE /api/repos/:owner/:slug`      owner or admin only
     - `GET/POST/DELETE /api/repos/:owner/:slug/members`  role management
     - `GET/POST /api/tokens` `DELETE /api/tokens/:id`  git token CRUD (show raw once, store hash only)
  2. Authorization helper: `fn can_read(user, repo) -> bool` and
     `fn can_write(user, repo) -> bool` — admin bypass, owner full, members by
     role, public = read for any authenticated user. (Single-user v1: owner
     does everything; helpers exist so multi-user is a data change, not a
     rewrite — see spec REQ-GIT-014.)
  3. Git transport auth enforcement: the backend owns a `GET
     /api/repos/:owner/:slug/auth` endpoint that the agent-facing git handler
     calls... (Implementation note: keep enforcement in the AGENT layer — the
     agent validates the Basic credential by calling `POST /internal/git/auth`
     on the backend, sending the token hash; backend answers `repo_id, role`.
     This keeps repo policy in one place. The backend→agent reverse check is
     NOT needed for v1 because the agent's git endpoints require this
     backend-issued capability.)
  4. Push authorization: `can_write` for receive-pack; `can_read` for
     upload-pack.
  5. Wire router.
  Accept: `cargo check -p dockpanel-backend` passes; create-repo flow works
  against a local DB+agent (manual).
  Depends on: T-P0-02, T-P1-04.

- [ ] **T-P1-06 · [M]** Agent: enforce backend-issued auth for git transport.
  Files: `panel/agent/src/services/git_http.rs` (edit), `panel/agent/src/routes/git_repos.rs` (edit).
  1. Agent validates the Basic credential of every git request by calling
     `POST /internal/git/auth` on the backend (Unix socket or HTTPS, same
     channel the agent already uses — see `services/agent.rs` on the backend
     side for the socket path/token conventions, mirrored in the agent's
     existing backend-calling code, e.g. `panel_update.rs` if it calls home).
  2. Cache successful auth results for 60s keyed by token-hash+repo to keep
     per-push latency low; invalidate on 401 from backend.
  3. Replace the T-P1-02 stub.
  Accept: clone with an invalid token → 401 from the agent; valid token from
  T-P1-05 CRUD → clone/push works.
  Depends on: T-P1-02, T-P1-05.

- [ ] **T-P1-07 · [S]** Backend: push event ingestion (post-receive equivalent).
  Files: `panel/backend/src/routes/git_repos.rs` (edit).
  1. Endpoint `POST /internal/git/push` — called by the agent after a
     successful receive-pack (agent sends repo id + ref updates
     `[(ref, old, new)]`).
  2. Backend:
     - updates `git_repos.size_bytes` (recompute via agent stats, fire-and-forget)
     - inserts activity log rows (`activity_logs`, existing service)
     - writes Notification Centre entries (`services/notifications.rs`)
     - returns 204.
  3. Agent calls it from the git_http receive-pack path (edit there, one line +
     payload struct).
  Accept: push → row in activity_logs + notification appears in the bell.
  Depends on: T-P1-05, T-P1-06.

---

## Phase 2 — PRs, review, issues

- [ ] **T-P2-01 · [M]** Backend: PR CRUD + number sequence.
  Files: `panel/backend/src/routes/git_prs.rs` (new), `panel/backend/src/routes/mod.rs` (edit).
  1. `POST /api/repos/:owner/:slug/pulls` — create PR (source_ref, target_ref,
     title, body); number = `MAX(number)+1` per repo inside one transaction
     (SELECT ... FOR UPDATE on the repo row to serialize).
  2. `GET /api/repos/:owner/:slug/pulls` list (filter state), `GET .../pulls/:number`
     detail, `PATCH .../pulls/:number` (title/body), `POST .../pulls/:number/close`.
  3. On create: validate source≠target, both refs exist (agent diff endpoint).
  4. Wire router.
  Accept: create/list/close PR round-trips on a local stack.
  Depends on: T-P1-04, T-P1-05.

- [ ] **T-P2-02 · [S]** Backend: PR diff via agent.
  Files: `panel/backend/src/routes/git_prs.rs` (edit).
  1. `GET .../pulls/:number/diff` → proxies agent `GET
     /agent/repos/:owner/:slug/diff/:a...:b` with `can_read` check.
  2. Response carries the raw patch + truncated flag.
  Accept: diff of a real PR renders (manual + later UI).
  Depends on: T-P2-01.

- [ ] **T-P2-03 · [M]** Backend: review + comment endpoints.
  Files: `panel/backend/src/routes/git_prs.rs` (edit).
  1. `POST .../pulls/:number/reviews` (approve / request-changes / comment)
     — upsert one decisive review per reviewer.
  2. `GET .../pulls/:number/reviews`.
  3. `POST .../pulls/:number/comments` + `GET` (top-level + line-anchored via
     optional file_path/line/commit_sha).
  4. Review state changes write activity + notifications.
  Accept: approve → changes_requested flow updates the decisive review row.
  Depends on: T-P2-01.

- [ ] **T-P2-04 · [M]** Backend: merge (rebase-only v1) + close/branch protection.
  Files: `panel/backend/src/routes/git_prs.rs` (edit), `panel/agent/src/routes/git_repos.rs` (edit for merge endpoint).
  1. Agent endpoint `POST /agent/repos/:owner/:slug/merge` — body `{source,
     target, committer_name, committer_email}`. Implementation: clone-free
     server-side merge is the honest v1 — `git fetch <repo> source:tmp-merge`,
     ... BUT bare repos cannot worktree-merge safely. **Chosen approach
     (spec REQ-GIT-013): rebase merge performed by the AGENT as:**
     1. resolve source head sha,
     2. `git rebase` is not available for bare; use `git cherry-pick` sequences
        is not bare-safe either.
     **Final decision: v1 merge = fast-forward only + squash-merge implemented
     via `git commit-tree` plumbing (bare-safe), rebase-merge deferred.**
     - FF: `git update-ref refs/heads/<target> <source_sha>` when `git
       merge-base --is-ancestor target source` says yes.
     - Squash: build a tree via `git read-tree` + `git commit-tree` sequence
       executed with `GIT_DIR` set (bare-safe, plumbing only).
     - Both refuse on conflict, returning the list of conflicting files from
       `git merge-tree` (plumbing, bare-safe).
  2. Backend `POST .../pulls/:number/merge` — checks: state open, ≥1 approval,
     0 unresolved changes-requested reviews, no conflict; then calls agent;
     sets state=merged + merged_commit; fires deploy-eligibility check.
  3. Branch protection v1: `git_repos.protect_default` boolean (add column in a
     small follow-up migration if not in T-P1-04) — when true, direct pushes to
     the default branch are refused at receive-pack with a clear pkt-line error.
  Accept: FF merge works; squash merge works; conflict → 409 with file list;
  protected branch push refused.
  Depends on: T-P2-01, T-P1-06.

- [ ] **T-P2-05 · [M]** Backend: issues CRUD + auto-close from commits.
  Files: `panel/backend/src/routes/git_issues.rs` (new), `panel/backend/src/routes/mod.rs` (edit).
  1. `POST/GET/PATCH /api/repos/:owner/:slug/issues`, `GET .../issues/:number`,
     close/reopen, comments `POST/GET .../issues/:number/comments`.
  2. Number sequence: same per-repo transaction pattern as PRs, but shared
     sequence with PRs? **Decision: separate sequences, `#N` namespace is
     scoped by kind** — spec REQ-GIT-010 resolves this as separate counters;
     frontend labels `#N` with kind prefix in UI ("PR #12", "Issue #7").
  3. Auto-close: in the T-P1-07 push ingestion, scan pushed commit subjects
     (`git log` via agent for the new shas) for `fixes|closes|resolves #N`
     (case-insensitive); close matching open issues with `closed_by_commit`;
     comment on the issue with the commit sha.
  4. Wire router.
  Accept: push with "fixes #1" closes issue 1 with a linking comment.
  Depends on: T-P1-04, T-P1-05, T-P1-07.

- [ ] **T-P2-06 · [S]** MCP: read-only git tools.
  Files: `panel/mcp/src/tools.rs` (edit).
  1. Add `list_repos`, `get_repo`, `list_commits`, `list_pull_requests`,
     `list_issues` following the exact `#[tool(description = ...)]` +
     `get_json` proxy pattern of the existing 38 tools.
  2. Zero mutating tools (the crate's stated invariant).
  Accept: tools appear in an MCP handshake; all are GET proxies.
  Depends on: T-P1-05, T-P2-01, T-P2-05.

---

## Phase 3 — Task management

- [ ] **T-P3-01 · [M]** Backend: task schema migration.
  Files: `panel/backend/migrations/20261123110000_task_management.sql` (new).
  1. Tables:
     - `projects (id, user_id FK, name, description, color, status
       ('active'|'archived'), default_repo_id FK NULL → git_repos, created_at,
       updated_at)`
     - `project_members (project_id, user_id, role ('admin'|'member'), UNIQUE)`
     - `tasks (id, project_id FK, title, description, status
       ('todo'|'in_progress'|'review'|'done'), priority
       ('low'|'medium'|'high'|'urgent'), assignee_id FK NULL, due_date NULL,
       sort_key BIGINT (fractional ranking for stable kanban drag), parent_task_id
       NULL self-FK, created_by, created_at, updated_at, completed_at NULL)`
     - `task_labels (id, project_id, name, color, UNIQUE(project_id,name))`
     - `task_label_map (task_id, label_id, UNIQUE)`
     - `task_comments (id, task_id, author_id, body, created_at)`
     - `task_links (id, task_id, kind ('commit'|'pr'|'issue'|'site'|'deployment'),
       external_id TEXT, url TEXT, created_at)` — the integration spine.
  2. Indexes on project_id+status, assignee, due_date.
  3. Applies clean on PG16.
  Accept: migration clean on fresh DB.
  Depends on: nothing.

- [ ] **T-P3-02 · [M]** Backend: project CRUD + membership.
  Files: `panel/backend/src/routes/projects.rs` (new), `panel/backend/src/routes/mod.rs` (edit).
  1. `POST/GET/PATCH/DELETE /api/projects` (+ `/:id` detail), members
     add/remove/role.
  2. Authorization: owner/admin bypass; members read+write tasks;
     non-members no access (404, not 403 — mirror the codebase's existing
     404-on-missing-permission pattern if one exists, else 403; grep
     `require_admin` helpers for the house style first).
  3. Wire router.
  Accept: CRUD round-trip; non-member gets rejected.
  Depends on: T-P3-01.

- [ ] **T-P3-03 · [M]** Backend: task CRUD + ordering + comments.
  Files: `panel/backend/src/routes/tasks.rs` (new), `panel/backend/src/routes/mod.rs` (edit).
  1. `POST /api/projects/:id/tasks`, `GET` (filters: status, assignee, label,
     due), `PATCH /api/tasks/:id` (status/priority/assignee/due/sort_key/parent),
     `DELETE`, `POST /api/tasks/:id/comments` + `GET`.
  2. Kanban reorder: fractional `sort_key` (midpoint of neighbors on drop;
     rebalance walk when precision runs out — document the constant).
  3. SSE: reuse `services/notifications.rs` patterns for realtime
     task-changed events on the project channel (the SSE shape exists in the
     Notification Centre — mirror it, do not invent a new bus).
  4. Wire router.
  Accept: move a task in the UI later; for now API reorder persists sort_key.
  Depends on: T-P3-01, T-P3-02.

- [ ] **T-P3-04 · [S]** Backend: git↔task link resolution.
  Files: `panel/backend/src/routes/tasks.rs` (edit).
  1. `POST /api/tasks/:id/links` create; `GET` list; `DELETE /api/links/:id`.
  2. Resolver: kind=commit → verify sha exists in the linked repo (agent);
     kind=pr/issue → verify + copy title/status snapshot; kind=site → validate
     site ownership.
  3. On PR merge (T-P2-04): auto-comment on tasks linked to that PR
     "Merged in <sha>"; on deploy success/failure of a linked site: same.
  Accept: link a commit, merge the PR, task shows the merged comment.
  Depends on: T-P3-03, T-P2-04.

- [ ] **T-P3-05 · [M]** Backend: task→issue bridge + `fixes #N` from task side.
  Files: `panel/backend/src/routes/tasks.rs` (edit).
  1. When a task is linked (kind=issue) to a git issue, closing the task
     closes the issue and vice versa (bidirectional sync, one-way queue to
     avoid loops — record origin of each transition).
  2. Task description auto-linking: `#N` (when project has default_repo_id)
     resolves to issue N on save (stored as link kind=issue, a comment notes
     the resolution) — mirrors GitHub autolink behavior.
  Accept: close task → issue closes; `#12` in a task description creates a link.
  Depends on: T-P3-04, T-P2-05.

- [ ] **T-P3-06 · [S]** MCP: read-only task tools.
  Files: `panel/mcp/src/tools.rs` (edit).
  1. `list_projects`, `list_tasks` (+ filter params), `get_task` — same
     `#[tool]` + `get_json` pattern.
  Accept: handshake exposes them; GET-only.
  Depends on: T-P3-03.

---

## Phase 4 — Frontend

- [ ] **T-P4-01 · [M]** Frontend: Git server pages.
  Files: `panel/frontend/src/pages/GitRepos.tsx` (new), `panel/frontend/src/pages/GitRepoDetail.tsx` (new), `panel/frontend/src/main.tsx` (edit).
  1. `GitRepos.tsx` — repo list: name, visibility badge, size, default branch,
     last commit subject + relative time, member avatars. Empty state, loading
     state, error state (R-27).
  2. `GitRepoDetail.tsx` — tabs: Commits (log list, click → commit detail with
     diff), Branches, Files (tree + raw view), Pull Requests (list + detail:
     diff + review buttons + comments), Issues (list + detail + comments),
     Settings (description, visibility, members, tokens link, danger zone
     delete).
  3. Clone URL copy box: `https://<panel-host>/git/<owner>/<slug>.git` with a
     token hint.
  4. Route registration in `main.tsx` (lazy import, same pattern as other
     pages).
  5. Theme compliance: use the existing theme variables only (6 themes × 3
     layouts must all render correctly — no hardcoded colors).
  Accept: all six themes render the pages without contrast failures; states
  (empty/loading/error) exist on every data view.
  Depends on: T-P1-05, T-P2-01, T-P2-05.

- [ ] **T-P4-02 · [M]** Frontend: PR review + issue UI flows.
  Files: `panel/frontend/src/pages/GitRepoDetail.tsx` (edit), shared components if needed.
  1. PR detail: diff viewer (unified, monospace, +/- line coloring from theme
     vars), approve / request-changes / comment actions, merge button
     (disabled with reason when unmet checks), merge-state badges.
  2. Issue detail: comment thread, close/reopen, `#N` autolink rendering.
  3. All forms submit with visible feedback (R-26); keyboard reachable (R-32).
  Accept: click-through evidence recorded per R-35 of antislop (list the
  elements exercised).
  Depends on: T-P4-01, T-P2-03, T-P2-04.

- [ ] **T-P4-03 · [M]** Frontend: Projects & kanban board.
  Files: `panel/frontend/src/pages/Projects.tsx` (new), `panel/frontend/src/pages/ProjectDetail.tsx` (new), `panel/frontend/src/main.tsx` (edit).
  1. `Projects.tsx` — project cards (name, color dot, open task counts by
     status, due-soon count), create/edit/archive.
  2. `ProjectDetail.tsx` — Kanban board: 4 columns (todo/in_progress/review/
     done), HTML5 drag-and-drop reorder within and across columns (persist
     sort_key), card shows priority marker, labels, assignee avatar, due date,
     link chips (commit/pr/issue/site).
  3. Task detail drawer: description (markdown), comments, checklist via
     parent/subtask, labels, assignee, due, links tab (add commit/pr/issue/site/
     deployment link).
  4. List view toggle for accessibility (drag-and-drop is not keyboard-
     operable by default; provide move up/down in list view — R-32).
  5. Empty/loading/error states everywhere.
  Accept: drag a card between columns persists after reload; keyboard users
  can reorder in list view; all themes render.
  Depends on: T-P3-03, T-P3-04.

- [ ] **T-P4-04 · [S]** Frontend: navigation entry points.
  Files: `panel/frontend/src/components/LayoutShell.tsx` or the layout components it delegates to (edit), `panel/frontend/src/components/CommandPalette` source (edit).
  1. Sidebar/compact/topbar nav: add "Git Repos" and "Projects" under the
     existing section grouping (read how Sites/Backups are grouped first).
  2. Command palette: register the new pages as destinations (Ctrl+K search).
  Accept: reachable in all 3 layouts; palette finds both.
  Depends on: T-P4-01, T-P4-03.

- [ ] **T-P4-05 · [S]** Frontend: notification deep links.
  Files: the notification list component (grep `Notifications.tsx` for link rendering), `panel/backend/src/services/notifications.rs` (edit, link payloads).
  1. Push/PR/issue/task events link to their object (the per-notification
     link pattern already exists in the Notification Centre — use it).
  Accept: clicking a "PR approved" notification opens that PR.
  Depends on: T-P4-01, T-P4-03, T-P1-07.

---

## Phase 5 — Integration wiring (deploy trigger, manifest, docs, tests)

- [ ] **T-P5-01 · [M]** Backend: push → Git Deploy auto-trigger.
  Files: `panel/backend/src/routes/git_repos.rs` (edit, push ingestion from T-P1-07), `panel/backend/src/routes/git_deploys.rs` (edit, export the trigger).
  1. Extend `git_deploys` table with `source_repo_id FK NULL` +
     `source_branch TEXT NULL` (small migration) — a Git Deploy config can
     point at a native repo instead of an external URL.
  2. Extract the deploy-trigger core from the existing webhook path in
     `git_deploys.rs` into a callable `pub async fn trigger_deploy(config_id,
     trigger: TriggerSource)` (refactor, no behavior change — the existing
     tests must keep passing).
  3. In push ingestion: find deploy configs where `source_repo_id = repo AND
     source_branch = pushed-ref`, call the trigger with source=GitPush.
  4. Deploy history rows record `triggered_by = git_push`.
  Accept: push to a watched branch starts a deploy; the history shows it;
  existing webhook deploys unaffected (their tests keep passing).
  Depends on: T-P1-07.

- [ ] **T-P5-02 · [S]** Backend: deploy outcome → notifications/links feedback.
  Files: `panel/backend/src/routes/git_deploys.rs` (edit), `panel/backend/src/routes/tasks.rs` (edit if link comments live there).
  1. On deploy success/failure for configs with `source_repo_id`, write
     Notification Centre entries that deep-link to the deploy page (pattern
     from T-P4-05).
  2. Tasks linked (kind=deployment) to that site/deploy get an auto-comment.
  Accept: deploy failure notification appears with a working link.
  Depends on: T-P5-01, T-P3-04.

- [ ] **T-P5-03 · [M]** Pin tests: auth + authorization.
  Files: `tests/git-token-auth-pin-e2e.sh` (new), `tests/git-repo-authz-pin-e2e.sh` (new).
  1. Token auth: correct token → 200; wrong/garbage → 401; disabled user's
     token → 401; `last_used_at` written.
  2. Authz: public repo read by non-member → allowed; private repo read by
     non-member → refused; write by read-role member → refused; write by
     write-role member → allowed; unauthenticated transport → 401.
  3. Register both in the regression-pins CI list.
  Accept: both green locally; registered for CI.
  Depends on: T-P1-05, T-P1-06.

- [ ] **T-P5-04 · [M]** Pin tests: git transport correctness.
  Files: `tests/git-smart-http-pin-e2e.sh` (new).
  1. Assert: info/refs advertisement content-type + pkt-line header; POST
     upload-pack round-trip clone; POST receive-pack push; push to protected
     branch refused with a pkt-line error message; traversal-guard repo path
     (`/agent/git/../../etc` → 400/404).
  2. Register in CI.
  Accept: green; registered.
  Depends on: T-P1-02, T-P2-04.

- [ ] **T-P5-05 · [M]** Pin tests: PR/issue/task business rules.
  Files: `tests/git-pr-review-gate-pin-e2e.sh` (new), `tests/git-issue-autoclose-pin-e2e.sh` (new), `tests/task-kanban-order-pin-e2e.sh` (new).
  1. PR gate: merge refused with 0 approvals; refused with unresolved
     changes-requested; allowed with 1 approval; conflict → 409 + file list.
  2. Issue auto-close: `fixes #N` in a pushed commit closes the issue +
     comment; non-matching text does nothing.
  3. Kanban order: fractional sort_key midpoint; rebalance trigger correct;
     concurrent move does not corrupt ordering (two PATCHes, final order
     consistent).
  4. Register all in CI.
  Accept: green; registered.
  Depends on: T-P2-04, T-P2-05, T-P3-03.

- [ ] **T-P5-06 · [M]** End-to-end journey test.
  Files: `tests/git-task-journey-e2e.sh` (new).
  1. The full loop on a real stack (fresh VPS profile like other e2e
     journeys): create user → create token → create repo → clone → commit
     "fixes #1" → push → issue 1 closed + notification → create task linked to
     repo → create PR → review approve → merge → deploy config on
     source_repo triggers → deploy completes → task link comments show merge +
     deploy → MCP tools list_repos/list_tasks return the objects.
  2. Register in the smoke-test CI workflow.
  Accept: the journey completes end-to-end on the CI runner profile.
  Depends on: T-P5-01, T-P2-04, T-P3-04, T-P5-03..05.

- [ ] **T-P5-07 · [S]** FEATURES.md manifest rows.
  Files: `FEATURES.md` (edit).
  1. Add rows to Hosting (Git Server: repos, smart-HTTP, tokens, PRs, reviews,
     issues, merge; backend/agent/frontend/DB columns filled correctly).
  2. Add a Task Management section/rows (projects, tasks, kanban, links,
     integrations).
  3. Update the Verified Metrics register ONLY with numbers that
     `tests/docs-claims-pin-e2e.sh` can derive (new route counts) — run the
     test after editing to learn the derived values; never hand-guess a number
     here.
  Accept: `bash tests/docs-claims-pin-e2e.sh` passes after the edit.
  Depends on: T-P1-05, T-P2-01, T-P2-05, T-P3-02, T-P3-03.

- [ ] **T-P5-08 · [S]** Docs guides.
  Files: `docs/guides/git-server.md` (new), `docs/guides/task-management.md` (new), `docs/SUMMARY.md` (edit).
  1. git-server.md: concepts (owner/slug, tokens, visibility), quickstart
     (clone/push), PR flow, branch protection, deploy integration, MCP tools,
     limitations (FF + squash merge only, rebase deferred, no wiki/LFS).
  2. task-management.md: projects, board, links, issue bridge, deploy
     comments, MCP tools.
  3. SUMMARY.md: register both.
  4. Honest limitations section in both (the repo's §Withdrawn Claims culture:
     say what v1 does not do).
  Accept: docs build (`mdbook build docs` or the repo's doc build command).
  Depends on: all prior.

- [ ] **T-P5-09 · [S]** CHANGELOG entry.
  Files: `CHANGELOG.md` (edit).
  1. New "Added" entries: Git Server (native, smart-HTTP, PRs/reviews/issues,
     merge FF+squash, branch protection), Task Management (projects, kanban,
     links, issue bridge), MCP tools, deploy-trigger integration.
  2. Honest Known-limitations notes (rebase merge deferred; no wiki/LFS; PR
     numbers and issue numbers are separate sequences).
  Accept: entry follows the file's existing format (read top entries first).
  Depends on: everything else.

---

## Completion checklist (whole plan)

- [ ] All Phase 0–5 tasks ticked.
- [ ] `cargo test` (workspace) + full pin suite green locally.
- [ ] `bash tests/docs-claims-pin-e2e.sh` passes (manifest/metrics honest).
- [ ] `report.md` filled with evidence (test names, manual click-through notes
      for the frontend tasks per R-35).
- [ ] No new claim anywhere (README/docs/FEATURES) that the §Withdrawn Claims
      section would have to retract later.
