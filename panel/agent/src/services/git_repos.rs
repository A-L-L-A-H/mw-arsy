use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::safe_cmd::safe_command;
use crate::site_lock::lock_site;

/// Root directory for bare git repositories hosted by the built-in git server.
/// Kept distinct from `/var/lib/arsy/git` (used by git_build deploy checkouts)
/// and inside `/var/lib/arsy` which is already in `arsy-agent.service` `ReadWritePaths`.
pub const GIT_REPOS_ROOT: &str = "/var/lib/arsy/git-repos";

const MAX_DIFF_BYTES: usize = 512 * 1024;
const MAX_RAW_BYTES: usize = 1024 * 1024;

/// Validate an owner or repository slug segment.
/// Must start with lowercase ASCII alphanumeric and contain only `[a-z0-9_-]`,
/// length 1..=64. Rejects `.`, `..`, `/`, `\`, leading `-`.
pub fn is_valid_segment(seg: &str) -> bool {
    if seg.is_empty() || seg.len() > 64 {
        return false;
    }
    let mut chars = seg.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Normalize a slug that may carry a trailing `.git` from smart-HTTP URLs.
pub fn normalize_slug(raw: &str) -> &str {
    raw.strip_suffix(".git").unwrap_or(raw)
}

/// Validate a git ref name (branch/tag/sha) so it cannot inject CLI flags or escape.
pub fn is_valid_ref(r: &str) -> bool {
    if r.is_empty() || r.len() > 256 || r.starts_with('-') || r.contains("..") || r.contains('\0') {
        return false;
    }
    r.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
}

/// Validate a relative path inside a repository tree.
pub fn is_valid_tree_path(p: &str) -> bool {
    if p.contains('\0') || p.starts_with('/') || p.starts_with('\\') {
        return false;
    }
    !p.split('/').any(|seg| seg == "..")
}

/// Resolve `<root>/<owner>/<slug>.git`, rejecting traversal or invalid segments.
pub fn repo_path(root: &Path, owner: &str, raw_slug: &str) -> Result<PathBuf, String> {
    let slug = normalize_slug(raw_slug);
    if !is_valid_segment(owner) {
        return Err(format!("Invalid repository owner: '{owner}'"));
    }
    if !is_valid_segment(slug) {
        return Err(format!("Invalid repository slug: '{slug}'"));
    }
    let candidate = root.join(owner).join(format!("{slug}.git"));
    if !candidate.starts_with(root) {
        return Err("Repository path escaped root".to_string());
    }
    Ok(candidate)
}

fn repo_lock_key(owner: &str, slug: &str) -> String {
    format!("git-repo:{owner}/{}", normalize_slug(slug))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoStats {
    pub size_bytes: u64,
    pub branch_count: usize,
    pub tag_count: usize,
    pub default_branch: Option<String>,
    pub head_commit: Option<String>,
    pub last_commit_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub sha: String,
    pub committed_at: String,
    pub subject: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitSummary {
    pub sha: String,
    pub short_sha: String,
    pub author_name: String,
    pub author_email: String,
    pub committed_at: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitDetail {
    pub sha: String,
    pub short_sha: String,
    pub author_name: String,
    pub author_email: String,
    pub committed_at: String,
    pub subject: String,
    pub body: String,
    pub patch: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TreeEntry {
    pub mode: String,
    pub kind: String,
    pub sha: String,
    pub size: Option<u64>,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiffResult {
    pub patch: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MergeResult {
    pub merged_commit: String,
    pub method: String,
}

async fn run_git(repo: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let repo_str = repo.to_string_lossy();
    let mut cmd = safe_command("git");
    cmd.arg("-C").arg(repo_str.as_ref()).args(args);
    cmd.kill_on_drop(true);
    tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output())
        .await
        .map_err(|_| "git command timed out".to_string())?
        .map_err(|e| format!("failed to execute git: {e}"))
}

fn dir_size_bytes(path: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Initialize a new bare repository at `<root>/<owner>/<slug>.git`.
pub async fn init_repo(
    root: &Path,
    owner: &str,
    slug: &str,
    default_branch: &str,
) -> Result<PathBuf, String> {
    if !is_valid_ref(default_branch) || default_branch.contains('/') {
        return Err("Invalid default branch name".to_string());
    }
    let path = repo_path(root, owner, slug)?;
    let _guard = lock_site(&repo_lock_key(owner, slug)).await;

    if path.exists() {
        return Err("Repository directory already exists on disk".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create owner git directory: {e}"))?;
    }

    let path_str = path.to_string_lossy().to_string();
    let mut cmd = safe_command("git");
    cmd.args(["init", "--bare", "--initial-branch", default_branch, &path_str]);
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to run git init: {e}"))?;
    if !out.status.success() {
        // Fallback for older git without --initial-branch
        let mut fallback = safe_command("git");
        fallback.args(["init", "--bare", &path_str]);
        let fb_out = fallback
            .output()
            .await
            .map_err(|e| format!("Failed to run git init: {e}"))?;
        if !fb_out.status.success() {
            std::fs::remove_dir_all(&path).ok();
            return Err(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&fb_out.stderr)
            ));
        }
        let head_ref = format!("refs/heads/{default_branch}");
        let _ = run_git(&path, &["symbolic-ref", "HEAD", &head_ref]).await;
    }

    // Enable smart-HTTP receive-pack on this bare repository
    let cfg = run_git(&path, &["config", "http.receivepack", "true"]).await?;
    if !cfg.status.success() {
        return Err(format!(
            "Failed to configure http.receivepack: {}",
            String::from_utf8_lossy(&cfg.stderr)
        ));
    }
    let _ = std::fs::write(
        path.join("description"),
        format!("{owner}/{}\n", normalize_slug(slug)),
    );

    Ok(path)
}

/// Remove a bare repository from disk.
pub async fn delete_repo(root: &Path, owner: &str, slug: &str) -> Result<(), String> {
    let path = repo_path(root, owner, slug)?;
    let _guard = lock_site(&repo_lock_key(owner, slug)).await;

    if !path.starts_with(root) {
        return Err("Refusing delete outside repository root".to_string());
    }
    if path.exists() {
        std::fs::remove_dir_all(&path)
            .map_err(|e| format!("Failed to remove repository directory: {e}"))?;
    }
    Ok(())
}

pub async fn repo_stats(root: &Path, owner: &str, slug: &str) -> Result<RepoStats, String> {
    let path = repo_path(root, owner, slug)?;
    if !path.exists() {
        return Err("Repository does not exist on disk".to_string());
    }

    let size_bytes = dir_size_bytes(&path);
    let branches = list_branches(root, owner, slug).await.unwrap_or_default();
    let tags_out = run_git(&path, &["tag", "-l"]).await?;
    let tag_count = if tags_out.status.success() {
        String::from_utf8_lossy(&tags_out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    } else {
        0
    };

    let head_sym = run_git(&path, &["symbolic-ref", "--short", "HEAD"]).await;
    let default_branch = head_sym.ok().and_then(|o| {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        } else {
            None
        }
    });

    let head_rev = run_git(&path, &["rev-parse", "HEAD"]).await;
    let head_commit = head_rev.ok().and_then(|o| {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        } else {
            None
        }
    });

    let last_commit_at = if head_commit.is_some() {
        run_git(&path, &["log", "-1", "--format=%aI", "HEAD"])
            .await
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() { None } else { Some(s) }
                } else {
                    None
                }
            })
    } else {
        None
    };

    Ok(RepoStats {
        size_bytes,
        branch_count: branches.len(),
        tag_count,
        default_branch,
        head_commit,
        last_commit_at,
    })
}

pub async fn list_branches(
    root: &Path,
    owner: &str,
    slug: &str,
) -> Result<Vec<BranchInfo>, String> {
    let path = repo_path(root, owner, slug)?;
    if !path.exists() {
        return Err("Repository does not exist on disk".to_string());
    }
    let out = run_git(
        &path,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)%00%(objectname)%00%(committerdate:iso8601-strict)%00%(contents:subject)",
            "refs/heads",
        ],
    )
    .await?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut items = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(4, '\0').collect();
        if parts.len() >= 3 {
            items.push(BranchInfo {
                name: parts[0].to_string(),
                sha: parts[1].to_string(),
                committed_at: parts[2].to_string(),
                subject: parts.get(3).copied().unwrap_or("").to_string(),
            });
        }
    }
    Ok(items)
}

pub async fn list_commits(
    root: &Path,
    owner: &str,
    slug: &str,
    git_ref: &str,
    limit: usize,
    skip: usize,
) -> Result<Vec<CommitSummary>, String> {
    if !is_valid_ref(git_ref) {
        return Err("Invalid git ref".to_string());
    }
    let path = repo_path(root, owner, slug)?;
    if !path.exists() {
        return Err("Repository does not exist on disk".to_string());
    }
    // Check if ref resolves (empty repository returns empty commit list cleanly)
    let rev = run_git(&path, &["rev-parse", "--verify", git_ref]).await?;
    if !rev.status.success() {
        return Ok(Vec::new());
    }

    let limit_str = limit.clamp(1, 200).to_string();
    let skip_str = skip.to_string();
    let out = run_git(
        &path,
        &[
            "log",
            git_ref,
            "--max-count",
            &limit_str,
            "--skip",
            &skip_str,
            "--format=%x1e%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%b",
        ],
    )
    .await?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commits = Vec::new();
    for rec in text.split('\x1e') {
        let rec = rec.trim();
        if rec.is_empty() {
            continue;
        }
        let parts: Vec<&str> = rec.splitn(7, '\0').collect();
        if parts.len() >= 6 {
            commits.push(CommitSummary {
                sha: parts[0].to_string(),
                short_sha: parts[1].to_string(),
                author_name: parts[2].to_string(),
                author_email: parts[3].to_string(),
                committed_at: parts[4].to_string(),
                subject: parts[5].to_string(),
                body: parts.get(6).copied().unwrap_or("").trim().to_string(),
            });
        }
    }
    Ok(commits)
}

pub async fn commit_detail(
    root: &Path,
    owner: &str,
    slug: &str,
    sha: &str,
) -> Result<CommitDetail, String> {
    if !is_valid_ref(sha) {
        return Err("Invalid commit sha".to_string());
    }
    let path = repo_path(root, owner, slug)?;
    if !path.exists() {
        return Err("Repository does not exist on disk".to_string());
    }
    let meta = run_git(
        &path,
        &[
            "show",
            "--no-patch",
            "--format=%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%b",
            sha,
        ],
    )
    .await?;
    if !meta.status.success() {
        return Err("Commit not found".to_string());
    }
    let meta_str = String::from_utf8_lossy(&meta.stdout);
    let parts: Vec<&str> = meta_str.trim().splitn(7, '\0').collect();
    if parts.len() < 6 {
        return Err("Malformed commit metadata".to_string());
    }

    let diff_out = run_git(&path, &["show", "--format=", "--patch", sha]).await?;
    let (patch, truncated) = truncate_utf8(&diff_out.stdout, MAX_DIFF_BYTES);

    Ok(CommitDetail {
        sha: parts[0].to_string(),
        short_sha: parts[1].to_string(),
        author_name: parts[2].to_string(),
        author_email: parts[3].to_string(),
        committed_at: parts[4].to_string(),
        subject: parts[5].to_string(),
        body: parts.get(6).copied().unwrap_or("").trim().to_string(),
        patch,
        truncated,
    })
}

pub async fn list_tree(
    root: &Path,
    owner: &str,
    slug: &str,
    git_ref: &str,
    subpath: &str,
) -> Result<Vec<TreeEntry>, String> {
    if !is_valid_ref(git_ref) || !is_valid_tree_path(subpath) {
        return Err("Invalid ref or path".to_string());
    }
    let path = repo_path(root, owner, slug)?;
    if !path.exists() {
        return Err("Repository does not exist on disk".to_string());
    }
    let rev = run_git(&path, &["rev-parse", "--verify", git_ref]).await?;
    if !rev.status.success() {
        return Ok(Vec::new());
    }
    let clean_sub = subpath.trim_matches('/');
    let spec = if clean_sub.is_empty() {
        format!("{git_ref}:")
    } else {
        format!("{git_ref}:{clean_sub}/")
    };
    let out = run_git(&path, &["ls-tree", "-l", &spec]).await?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries = Vec::new();
    for line in text.lines() {
        // Format: <mode> SP <type> SP <object> SP <size>\t<file>
        let Some((left, name)) = line.split_once('\t') else {
            continue;
        };
        let cols: Vec<&str> = left.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let size = cols[3].parse::<u64>().ok();
        let entry_path = if clean_sub.is_empty() {
            name.to_string()
        } else {
            format!("{clean_sub}/{name}")
        };
        entries.push(TreeEntry {
            mode: cols[0].to_string(),
            kind: cols[1].to_string(),
            sha: cols[2].to_string(),
            size,
            name: name.to_string(),
            path: entry_path,
        });
    }
    // Sort directories first, then files alphabetically
    entries.sort_by(|a, b| {
        let ad = a.kind == "tree";
        let bd = b.kind == "tree";
        bd.cmp(&ad).then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

pub async fn raw_blob(
    root: &Path,
    owner: &str,
    slug: &str,
    git_ref: &str,
    file_path: &str,
) -> Result<(Vec<u8>, bool), String> {
    if !is_valid_ref(git_ref) || !is_valid_tree_path(file_path) || file_path.is_empty() {
        return Err("Invalid ref or file path".to_string());
    }
    let path = repo_path(root, owner, slug)?;
    if !path.exists() {
        return Err("Repository does not exist on disk".to_string());
    }
    let spec = format!("{git_ref}:{}", file_path.trim_start_matches('/'));
    let out = run_git(&path, &["cat-file", "blob", &spec]).await?;
    if !out.status.success() {
        return Err("File not found at ref".to_string());
    }
    if out.stdout.len() > MAX_RAW_BYTES {
        Ok((out.stdout[..MAX_RAW_BYTES].to_vec(), true))
    } else {
        Ok((out.stdout, false))
    }
}

pub async fn diff_refs(
    root: &Path,
    owner: &str,
    slug: &str,
    base_ref: &str,
    head_ref: &str,
) -> Result<DiffResult, String> {
    if !is_valid_ref(base_ref) || !is_valid_ref(head_ref) {
        return Err("Invalid ref".to_string());
    }
    let path = repo_path(root, owner, slug)?;
    if !path.exists() {
        return Err("Repository does not exist on disk".to_string());
    }
    let range = format!("{base_ref}...{head_ref}");
    let out = run_git(&path, &["diff", &range]).await?;
    if !out.status.success() {
        // Fallback to two-dot diff if unrelated histories / no merge base
        let range2 = format!("{base_ref}..{head_ref}");
        let out2 = run_git(&path, &["diff", &range2]).await?;
        if !out2.status.success() {
            return Err(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&out2.stderr)
            ));
        }
        let (patch, truncated) = truncate_utf8(&out2.stdout, MAX_DIFF_BYTES);
        return Ok(DiffResult { patch, truncated });
    }
    let (patch, truncated) = truncate_utf8(&out.stdout, MAX_DIFF_BYTES);
    Ok(DiffResult { patch, truncated })
}

/// Collect commit subjects+bodies across a pushed ref range (for `fixes #N` parsing).
pub async fn commit_messages_in_range(
    root: &Path,
    owner: &str,
    slug: &str,
    old_sha: &str,
    new_sha: &str,
) -> Result<Vec<(String, String)>, String> {
    if !is_valid_ref(old_sha) || !is_valid_ref(new_sha) {
        return Err("Invalid commit range".to_string());
    }
    let path = repo_path(root, owner, slug)?;
    if !path.exists() {
        return Err("Repository does not exist on disk".to_string());
    }
    if new_sha.chars().all(|c| c == '0') {
        return Ok(Vec::new());
    }
    let range = if old_sha.chars().all(|c| c == '0') {
        new_sha.to_string()
    } else {
        format!("{old_sha}..{new_sha}")
    };
    let out = run_git(
        &path,
        &["log", &range, "--max-count=100", "--format=%x1e%H%x00%B"],
    )
    .await?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut res = Vec::new();
    for rec in text.split('\x1e') {
        let rec = rec.trim();
        if rec.is_empty() {
            continue;
        }
        if let Some((sha, msg)) = rec.split_once('\0') {
            res.push((sha.to_string(), msg.trim().to_string()));
        }
    }
    Ok(res)
}

#[derive(Debug)]
pub enum MergeFailure {
    Conflict(Vec<String>),
    Invalid(String),
}

/// Bare-safe merge using git plumbing only (fast-forward or squash).
/// Rebase merge is explicitly deferred in v1.
pub async fn merge_refs(
    root: &Path,
    owner: &str,
    slug: &str,
    source_branch: &str,
    target_branch: &str,
    method: &str,
    commit_message: &str,
    author_name: &str,
    author_email: &str,
) -> Result<MergeResult, MergeFailure> {
    if !is_valid_ref(source_branch) || !is_valid_ref(target_branch) {
        return Err(MergeFailure::Invalid("Invalid branch name".to_string()));
    }
    if source_branch == target_branch {
        return Err(MergeFailure::Invalid(
            "Source and target branches must differ".to_string(),
        ));
    }
    if method != "ff" && method != "squash" {
        return Err(MergeFailure::Invalid(format!(
            "Unsupported merge method '{method}' (supported: ff, squash)"
        )));
    }

    let path = repo_path(root, owner, slug).map_err(MergeFailure::Invalid)?;
    let _guard = lock_site(&repo_lock_key(owner, slug)).await;

    let src_ref = format!("refs/heads/{source_branch}");
    let dst_ref = format!("refs/heads/{target_branch}");

    let src_out = run_git(&path, &["rev-parse", "--verify", &src_ref])
        .await
        .map_err(MergeFailure::Invalid)?;
    if !src_out.status.success() {
        return Err(MergeFailure::Invalid(format!(
            "Source branch '{source_branch}' not found"
        )));
    }
    let src_sha = String::from_utf8_lossy(&src_out.stdout).trim().to_string();

    let dst_out = run_git(&path, &["rev-parse", "--verify", &dst_ref])
        .await
        .map_err(MergeFailure::Invalid)?;
    if !dst_out.status.success() {
        return Err(MergeFailure::Invalid(format!(
            "Target branch '{target_branch}' not found"
        )));
    }
    let dst_sha = String::from_utf8_lossy(&dst_out.stdout).trim().to_string();

    if method == "ff" {
        let is_anc = run_git(&path, &["merge-base", "--is-ancestor", &dst_sha, &src_sha])
            .await
            .map_err(MergeFailure::Invalid)?;
        if !is_anc.status.success() {
            return Err(MergeFailure::Invalid(
                "Target branch cannot fast-forward to source; use squash merge".to_string(),
            ));
        }
        let upd = run_git(&path, &["update-ref", &dst_ref, &src_sha, &dst_sha])
            .await
            .map_err(MergeFailure::Invalid)?;
        if !upd.status.success() {
            return Err(MergeFailure::Invalid(format!(
                "update-ref failed: {}",
                String::from_utf8_lossy(&upd.stderr)
            )));
        }
        return Ok(MergeResult {
            merged_commit: src_sha,
            method: "ff".to_string(),
        });
    }

    // Squash merge via bare-safe `git merge-tree --write-tree` + `git commit-tree`
    let mt = run_git(&path, &["merge-tree", "--write-tree", &dst_sha, &src_sha])
        .await
        .map_err(MergeFailure::Invalid)?;
    let mt_out = String::from_utf8_lossy(&mt.stdout);
    if !mt.status.success() {
        let mut conflicts = Vec::new();
        for line in mt_out.lines() {
            if line.starts_with("CONFLICT ") {
                if let Some(file) = line.rsplit(" in ").next() {
                    conflicts.push(file.trim().to_string());
                }
            }
        }
        if conflicts.is_empty() {
            let err_msg = String::from_utf8_lossy(&mt.stderr).trim().to_string();
            if err_msg.contains("unrecognized argument") || err_msg.contains("usage:") {
                return Err(MergeFailure::Invalid(
                    "Installed git version does not support merge-tree --write-tree (requires git >= 2.38)"
                        .to_string(),
                ));
            }
            conflicts.push("(merge conflict)".to_string());
        }
        return Err(MergeFailure::Conflict(conflicts));
    }

    let tree_oid = mt_out
        .lines()
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| MergeFailure::Invalid("merge-tree returned empty tree OID".to_string()))?;

    let path_str = path.to_string_lossy().to_string();
    let mut cmd = safe_command("git");
    cmd.args([
        "-C",
        &path_str,
        "commit-tree",
        &tree_oid,
        "-p",
        &dst_sha,
        "-m",
        commit_message,
    ])
    .env("GIT_AUTHOR_NAME", author_name)
    .env("GIT_AUTHOR_EMAIL", author_email)
    .env("GIT_COMMITTER_NAME", author_name)
    .env("GIT_COMMITTER_EMAIL", author_email);
    cmd.kill_on_drop(true);

    let ct = tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output())
        .await
        .map_err(|_| MergeFailure::Invalid("commit-tree timed out".to_string()))?
        .map_err(|e| MergeFailure::Invalid(format!("commit-tree failed: {e}")))?;

    if !ct.status.success() {
        return Err(MergeFailure::Invalid(format!(
            "commit-tree failed: {}",
            String::from_utf8_lossy(&ct.stderr)
        )));
    }
    let new_sha = String::from_utf8_lossy(&ct.stdout).trim().to_string();
    let upd = run_git(&path, &["update-ref", &dst_ref, &new_sha, &dst_sha])
        .await
        .map_err(MergeFailure::Invalid)?;
    if !upd.status.success() {
        return Err(MergeFailure::Invalid(format!(
            "update-ref failed: {}",
            String::from_utf8_lossy(&upd.stderr)
        )));
    }

    Ok(MergeResult {
        merged_commit: new_sha,
        method: "squash".to_string(),
    })
}

fn truncate_utf8(bytes: &[u8], max: usize) -> (String, bool) {
    if bytes.len() <= max {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    } else {
        let mut s = String::from_utf8_lossy(&bytes[..max]).into_owned();
        s.push_str("\n\n... [truncated]");
        (s, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_validation_blocks_traversal() {
        assert!(is_valid_segment("my-repo_1"));
        assert!(is_valid_segment("a"));
        assert!(!is_valid_segment(""));
        assert!(!is_valid_segment(".."));
        assert!(!is_valid_segment("../etc"));
        assert!(!is_valid_segment("foo/bar"));
        assert!(!is_valid_segment("-leading"));
        assert!(!is_valid_segment("Upper"));
        assert!(!is_valid_segment("a\\b"));
    }

    #[test]
    fn repo_path_stays_inside_root() {
        let root = Path::new("/var/lib/arsy/git-repos");
        let ok = repo_path(root, "alice", "demo.git").unwrap();
        assert_eq!(ok, PathBuf::from("/var/lib/arsy/git-repos/alice/demo.git"));
        assert!(repo_path(root, "..", "etc").is_err());
        assert!(repo_path(root, "alice", "../../etc").is_err());
    }
}
