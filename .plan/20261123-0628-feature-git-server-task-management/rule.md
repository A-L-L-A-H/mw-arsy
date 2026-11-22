# Rules — Git Server + Task Management (DockPanel fork)

Hard rules the implementer must obey. These override general preferences for
this task. Violating any of them is a failed task even if the code works.

## Security

1. **Every git subprocess routes through `safe_cmd`** (`panel/agent/src/safe_cmd.rs`).
   No `Command::new("git")` anywhere else. Enforced by `tests/git-spawn-discipline-pin-e2e.sh`.
2. **No credential ever stored in plaintext.** Git tokens: SHA-256 hash only
   (mirror `api_keys.rs`), raw shown once at creation. GitHub-style tokens get
   the same AES-256-GCM storage treatment the codebase already applies
   (`encrypt_stored_token` in git_deploys.rs) for any future external token.
3. **Constant-time comparison for token hashes**, the same primitive
   `api_keys.rs`/`auth.rs` already uses. No new crypto invented.
4. **Path traversal is a hard gate.** `owner` and `slug` match
   `^[a-z0-9][a-z0-9-_]*$` and resolve strictly inside the repo root. Every
   agent handler re-validates; never trust the backend.
5. **The Docker-socket rule is not negotiable** (mirrors the Portainer
   withdrawal in FEATURES.md §Withdrawn Claims): no git hook, CI step, or app
   template introduced by this plan may mount the host Docker socket.
6. **Receive-pack auth is enforced in the agent**, not only at the backend —
   the agent calls `POST /internal/git/auth` per request (60s cache). A
   backend outage must fail closed (refuse pushes), never fail open.
7. **No mutation via MCP.** The MCP crate's zero-mutating-tools invariant
   holds. New MCP tools are GET proxies only.
8. **Secrets never in logs.** Push payloads, token values, and auth headers
   are excluded from agent/backend logs and from error messages returned to
   clients.

## Compatibility

9. **Existing behavior is frozen.** The `trigger_deploy` extraction (T-P5-01)
   is a pure refactor: all existing git_deploys pin tests must pass unmodified.
   Webhook-based deploys keep working throughout.
10. **No breaking API changes.** New routes only; existing route shapes
    untouched. New migrations only append (the codebase cannot edit applied
    migrations — sqlx checksums, see the §Withdrawn Claims note on
    `20260805000000_client_role.sql`).
11. **All 6 themes and 3 layouts must render every new page correctly.** No
    hardcoded colors; theme variables only.
12. **Agent version gating** follows the codebase's `MIN_AGENT` pattern
    (see `BACKUP_CARRY_MIN_AGENT` in sites.rs): new agent endpoints used by
    the backend must check the agent's reported version and degrade with a
    clear message, not crash.

## Quality

13. **Every P0/P1 fix and every new authz boundary gets a pin test** in
    `tests/`, registered in CI the same way the existing 150 tests are.
14. **FEATURES.md is updated in the same PR as the feature** it describes, and
    `tests/docs-claims-pin-e2e.sh` must pass after the edit. Metrics numbers
    are never hand-written; only derived values from that test.
15. **No claim without a mechanism** (the repo's core value): every capability
    stated in docs must have a working code path, a test, or an explicit
    limitation note. If it cannot be honestly claimed, it is not claimed.
16. **Honest failure modes**: merge conflicts return the conflicting file list;
    protected-branch pushes return a pkt-line error message; unavailable
    backends fail closed. No silent lies, no 500-with-no-context.
17. **Frontend delivery gate**: every new page ships empty/loading/error
    states, keyboard navigation, visible focus, and is click-through-verified
    (recorded in report.md per element). Drag-and-drop features must have a
    keyboard-operable equivalent (list-view reorder).
18. **`cargo clippy -- -D warnings` and `cargo fmt --check` pass** on all four
    Rust crates touched, matching repo CI gates.
19. **No drive-by refactors.** Only the files this plan names change. If a
    needed change is outside the plan, stop and ask.

## Forbidden actions

20. **Do not touch** `.github/workflows/release.yml` signing steps, existing
    migrations, `SECURITY.md` claims, or the §Withdrawn Claims table.
21. **Do not introduce** new external dependencies without listing them in
    design.md with justification (prefer the crates already in the workspace:
    axum, sqlx, serde, sha2, hex, rand).
22. **Do not implement** rebase merge, wiki, LFS, code search, or CI runner in
    v1 — they are explicitly out of scope (spec NFR-SC-2). A PR that adds them
    is rejected.
23. **Do not run** production writes, deploys, or push to any remote. All
    verification happens on the local dev stack (docker-compose postgres on
    5450 + local agent) per CONTRIBUTING.md.
