# Automation

DockPanel's repository automation — what runs by itself, what it touches, and how to
override each piece by hand.

## The release ladder

```
feat branch ──PR──▶ dev (default branch)
                      │  CI + CodeQL run here
                      ▼ merge
                    release-train.yml computes the next version from the commit
                    history (conventional commits: `!:` → major, `feat:` → minor,
                    anything else → patch), writes the version files, the two doc
                    stamps and the CHANGELOG section, commits that on `dev`, and
                    opens the release PR `dev → main`.
                      │  main-gate ruleset: the FULL CI matrix must pass
                      ▼ auto-merge (squash)
                    the train tags vX.Y.Z on the merge commit
                      ▼ release.yml (the existing tag-triggered pipeline)
                    changelog → regression pins → static musl builds → SBOM +
                    checksums → Sigstore signing → GitHub Release → install
                    smoke test
```

Consequences worth knowing:

- Every merge into `main` is a release. There is no "batch".
- A merge whose commits are all `chore:`/`fix:`/`docs:` bumps the patch version.
- `feat:` bumps the minor; a `!:` (breaking) subject bumps the major.
- The train never merges its own PR against a red matrix — `main` cannot
  regress past the same gates a human PR faces.

## Required checks (the main gate)

Both branch rulesets require the full CI matrix: lint (Rust + frontend), builds,
unit tests, the security audits, `Documentation Claims`, `Nginx Response Contract`,
`Update & Rollback Contract`, the agent/backup/domain/ownership/container pin
suites, `A Pinned Agent Identity Refuses An Impostor`, and `Every Regression Pin`.
A green matrix is the only path into `main`.

## Manual override (per automation)

| Automation | Override |
|---|---|
| Release train | Disable `Release Train` in Actions, then cut a release by hand: bump the 5 version files + 2 doc stamps + CHANGELOG, push to `dev`, merge the PR, `git tag vX.Y.Z && git push origin vX.Y.Z`. |
| Dependabot auto-merge | Comment `@dependabot` commands on the PR (`@dependabot squash and merge`, `@dependabot close`), or close the PR. A PR on the deny list in `.github/workflows/auto-merge.yml` (`DENY_LIST`) never merges automatically. |
| CI on `dev` | Push directly to `dev`; CI runs. Rule approval is not required on `dev` for the owner (ruleset bypass). |

## Failure detection

A failed train run opens or updates one issue titled
`automation/release-failure: run <id>` with the run URL. The next successful run
closes it. Dependabot auto-merge skips leave a comment + the `automerge-skipped`
label on the PR itself.

## Kill switch

Disable the workflows in the Actions UI (Workflows → select → "⋯" → Disable).
The rest of the repo works unchanged; releases fall back to the manual ladder
above.
